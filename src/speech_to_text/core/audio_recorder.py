import logging
import array
import math
import time
import threading
import os
import signal
import subprocess
import tempfile
import wave
from pathlib import Path

logger = logging.getLogger(__name__)


class AudioRecorder:
    """One capture at a time, with a private file for each dictation."""

    def __init__(self, sample_rate=16000, format='S16_LE', channels=1,
                 output_file='/tmp/stt_recording.wav', device='default', pipewire_node='', on_level=None):
        self.sample_rate = sample_rate
        self.format = format
        self.channels = channels
        self.output_template = output_file
        self.output_file = output_file
        self.device = device
        self.pipewire_node = pipewire_node
        self.process = None
        self._stderr = None
        self.on_level = on_level
        self._monitor_stop = threading.Event()
        self._monitor = None

    def start(self) -> None:
        if self.process is not None:
            raise RuntimeError('A recording is already active')
        template = Path(self.output_template).expanduser()
        template.parent.mkdir(parents=True, exist_ok=True)
        fd, self.output_file = tempfile.mkstemp(prefix=template.stem + '-', suffix='.wav', dir=template.parent)
        os.close(fd)
        self._stderr = tempfile.TemporaryFile()
        cmd = ['arecord', '-q', '-D', self.device, '-f', self.format,
               '-r', str(self.sample_rate), '-c', str(self.channels), self.output_file]
        env = os.environ.copy()
        if self.pipewire_node:
            env['PIPEWIRE_NODE'] = self.pipewire_node
        self._started = time.monotonic()
        logger.info('Capture started: id=%s device=%s node=%s rate=%d channels=%d',
                    Path(self.output_file).stem, self.device, self.pipewire_node or 'system default',
                    self.sample_rate, self.channels)
        try:
            self.process = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=self._stderr, env=env, umask=0o077)
            # Detect immediate device errors. No fixed sleep after successful startup.
            try:
                self.process.wait(timeout=0.05)
            except subprocess.TimeoutExpired:
                if self.on_level:
                    self._monitor_stop.clear()
                    self._monitor = threading.Thread(target=self._monitor_levels,
                        args=(self.output_file,), name='capture-levels', daemon=True)
                    self._monitor.start()
                return
            raise RuntimeError(self._error() or 'Audio capture exited before recording started')
        except Exception:
            self.process = None
            self._close_stderr()
            self.discard()
            raise

    def _monitor_levels(self, path):
        ready = False
        try:
            while not self._monitor_stop.wait(0.1):
                # arecord writes samples in blocks. Reading the most recent block
                # reports actual signal, including a silent or disconnected input.
                size = os.path.getsize(path)
                if size <= 128:
                    continue
                if not ready:
                    logger.info('First audio samples: id=%s startup_ms=%.0f',
                                Path(path).stem, (time.monotonic()-self._started)*1000)
                    ready = True
                with open(path, 'rb') as stream:
                    stream.seek(max(128, size-3200))
                    data = stream.read(3200)
                samples = array.array('h', data[:len(data)//2*2])
                rms = math.sqrt(sum(x*x for x in samples)/max(1,len(samples))) / 32768
                db = 20*math.log10(max(rms, 1e-6))
                self.on_level(path, max(0, min(1, (db+60)/60)), db)
        except OSError:
            if not self._monitor_stop.is_set():
                logger.debug('Capture level monitor stopped', exc_info=True)

    def _stop_monitor(self):
        self._monitor_stop.set()
        if self._monitor and self._monitor is not threading.current_thread():
            self._monitor.join(timeout=1)

    def _error(self):
        self._stderr.seek(0)
        return self._stderr.read(4096).decode(errors='replace').strip()

    def _close_stderr(self):
        if self._stderr:
            self._stderr.close()
            self._stderr = None

    def stop(self) -> bool:
        if self.process is None:
            return False
        process = self.process
        self._stop_monitor()
        try:
            if process.poll() is not None:
                raise RuntimeError(self._error() or 'Audio capture stopped unexpectedly')
            # SIGINT lets arecord finalize the WAV header.
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
                raise RuntimeError('Audio capture did not stop cleanly')
            with wave.open(self.output_file, 'rb') as audio:
                frames = audio.getnframes()
                if frames < audio.getframerate() * 0.1:
                    return False
                # Report a broken/muted input without loading Whisper or running ffmpeg.
                has_signal = False
                energy = samples_count = peak = 0
                while chunk := audio.readframes(8192):
                    if self.format == 'S16_LE':
                        samples = array.array('h', chunk)
                        peak = max(peak, max((abs(x) for x in samples), default=0))
                        energy += sum(x*x for x in samples)
                        samples_count += len(samples)
                        has_signal = has_signal or peak > 0
                    else:
                        has_signal = True
                rms = math.sqrt(energy / max(1, samples_count)) / 32768
                logger.info('Capture complete: id=%s audio=%.3fs peak=%d rms_db=%.1f',
                            Path(self.output_file).stem, frames/audio.getframerate(), peak,
                            20*math.log10(max(rms, 1e-6)))
                if not has_signal:
                    raise RuntimeError('Microphone returned silence. Check the selected microphone and mute setting in Settings → Input.')
            return True
        finally:
            self.process = None
            self._close_stderr()

    def discard(self):
        Path(self.output_file).unlink(missing_ok=True)
