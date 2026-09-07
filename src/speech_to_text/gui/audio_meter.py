import array
import math
import os
import subprocess
import threading

from speech_to_text.gui.gtk import GLib


class MicrophoneTest:
    """An eight-second level check. Samples stay in memory and are never transcribed."""

    def __init__(self, device, node, callback):
        self.device, self.node, self.callback = device, node, callback
        self._stop = threading.Event()
        self._lock = threading.Lock()
        self._process = None
        self._thread = threading.Thread(target=self._run, name='microphone-test', daemon=True)

    def start(self):
        self._thread.start()

    def stop(self):
        self._stop.set()
        with self._lock:
            if self._process and self._process.poll() is None:
                self._process.terminate()

    def _emit(self, level, message, done=False):
        if not self._stop.is_set():
            GLib.idle_add(self.callback, level, message, done)

    def _run(self):
        env = os.environ.copy()
        if self.node:
            env['PIPEWIRE_NODE'] = self.node
        peak = 0
        try:
            with self._lock:
                if self._stop.is_set():
                    return
                self._process = subprocess.Popen(
                    ['arecord', '-q', '-D', self.device, '-t', 'raw', '-f', 'S16_LE',
                     '-r', '16000', '-c', '1', '-d', '8'], env=env,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            # Independent timeout also covers a server that never supplies samples.
            timer = threading.Timer(10, self._terminate)
            timer.daemon = True
            timer.start()
            try:
                while not self._stop.is_set():
                    data = self._process.stdout.read(3200)
                    if not data:
                        break
                    samples = array.array('h', data[:len(data)//2*2])
                    rms = math.sqrt(sum(x*x for x in samples)/max(1, len(samples))) / 32768
                    peak = max(peak, rms)
                    db = 20 * math.log10(max(rms, 1e-6))
                    self._emit(max(0, min(1, (db+60)/60)), f'{db:.0f} dB · Speak normally to check your microphone')
                self._process.wait(timeout=2)
                detail = self._process.stderr.read(4096).decode(errors='replace').strip()
                if self._process.returncode != 0:
                    self._emit(0, detail or 'Microphone test stopped unexpectedly.', True)
                elif peak == 0:
                    self._emit(0, 'No signal. Choose a microphone instead of a loopback, or check mute.', True)
                else:
                    self._emit(0, 'Microphone signal detected. You are ready to dictate.', True)
            finally:
                timer.cancel()
        except Exception as exc:
            self._emit(0, f'Microphone test failed: {exc}', True)
        finally:
            self._terminate()
            if self._process:
                self._process.wait(timeout=2)
                self._process.stdout.close()
                self._process.stderr.close()

    def _terminate(self):
        with self._lock:
            if self._process and self._process.poll() is None:
                self._process.kill()
