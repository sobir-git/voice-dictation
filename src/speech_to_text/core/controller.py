import logging
import queue
import threading
import time
from pathlib import Path

from speech_to_text.core.audio_preprocessor import cleanup_processed, preprocess_audio
from speech_to_text.core.audio_recorder import AudioRecorder
from speech_to_text.core.config import Config
from speech_to_text.core.device_detector import DeviceDetector
from speech_to_text.core.hotkey_listener import HotkeyListener
from speech_to_text.core.notifications import Notifier
from speech_to_text.core.text_output import TextOutput
from speech_to_text.core.transcriber import Transcriber

logger = logging.getLogger(__name__)


class SpeechToTextController:
    MAX_PENDING = 4

    def __init__(self, config: Config, on_state=None, on_transcription=None, on_error=None, on_level=None):
        self.config = config
        self.on_state = on_state
        self.on_transcription = on_transcription
        self.on_error = on_error
        self.on_level = on_level
        self.capture_ready = False
        self._generation = 0
        self.running = True
        self.is_listening = False
        self.is_recording = False
        self.is_processing = False
        self._lock = threading.RLock()
        self._recording_done = threading.Event()
        self._recording_done.set()
        self._outputting = False
        self._jobs = queue.Queue()
        self._pending = 0
        self._worker = None
        self._listener = None
        self.trigger_key = config.get('input', 'trigger_key')
        self.recorder = self._build_recorder()
        self.transcriber = self._build_transcriber()
        self.text_output = TextOutput(method=config.get('output', 'method'))
        self.notifier = Notifier(enabled=config.get('notifications', 'enabled'),
                                 audio_feedback=config.get('notifications', 'audio_feedback'))

    def _build_recorder(self):
        return AudioRecorder(sample_rate=self.config.get('audio', 'sample_rate'),
                             format=self.config.get('audio', 'format'),
                             channels=self.config.get('audio', 'channels'),
                             output_file=self.config.get('audio', 'temp_file'),
                             device=self.config.get('audio', 'device', 'default'),
                             pipewire_node=self.config.get('audio', 'pipewire_node', ''),
                             on_level=self._on_level)

    def _on_level(self, path, level, db):
        if self.is_recording and path == self.recorder.output_file:
            self.capture_ready = True
            if self.on_level:
                self.on_level(level, db)

    def cancel(self):
        with self._lock:
            self._generation += 1
            if self.is_recording:
                self.is_recording = False
                try:
                    self.recorder.stop()
                except Exception:
                    logger.debug('Capture stopped during cancellation', exc_info=True)
                finally:
                    self.recorder.discard()
            self._recording_done.set()
            self.capture_ready = False
            logger.info('Cancellation requested: pending=%d', self._pending)
            self._emit_state()

    def _build_transcriber(self):
        return Transcriber(model_name=self.config.get('transcription', 'model'),
                           compute_type=self.config.get('transcription', 'compute_type'),
                           language=self.config.get('transcription', 'language'),
                           beam_size=self.config.get('transcription', 'beam_size'),
                           vad_filter=self.config.get('transcription', 'vad_filter'))

    def _start_listener(self):
        self._listener = HotkeyListener(
            trigger_key=self.trigger_key, on_key_down=self.start_recording,
            on_key_up=self.stop_recording, on_error=lambda e: self._emit_error(str(e)),
            is_running=lambda: self.running, is_enabled=lambda: self.is_listening)
        self._listener.validate()
        self._listener.start(daemon=True)

    def start(self):
        ok, message = DeviceDetector.check_permissions()
        if not ok:
            self._emit_error(message)
            return False
        try:
            self.is_listening = True
            self._start_listener()
        except Exception as exc:
            self.is_listening = False
            self._emit_error(f'Failed to start hotkey listener: {exc}')
            return False
        self._emit_state()
        threading.Thread(target=self._preload_model, args=(self.transcriber,), name='model-loader', daemon=True).start()
        return True

    def stop(self):
        with self._lock:
            self.running = False
            self._recording_done.set()
            self.is_listening = False
            if self.is_recording:
                self.stop_recording()
        if self._listener:
            self._listener.stop()
        # The worker discards queued jobs and suppresses output after shutdown.
        if self._worker:
            self._jobs.put(None)
            self._worker.join(timeout=5)
        self._emit_state()

    def set_listening(self, enabled):
        with self._lock:
            self.is_listening = enabled
            if not enabled and self.is_recording:
                self.stop_recording()
            self._emit_state()

    def reload_config(self):
        # Do not switch model, microphone, or output beneath an active dictation.
        with self._lock:
            if self.is_recording or self._pending:
                raise RuntimeError('Wait for dictation to finish, then save settings again.')
            enabled = self.is_listening
            self.is_listening = False
        if self._listener:
            self._listener.stop()
        try:
            candidate = Config()
            old = self.config
            self.config = candidate
            try:
                recorder = self._build_recorder()
                model_changed = candidate.data['transcription'] != old.data['transcription']
                transcriber = self._build_transcriber() if model_changed else self.transcriber
                output = (TextOutput(method=candidate.get('output', 'method'))
                          if candidate.get('output', 'method') != old.get('output', 'method')
                          else self.text_output)
            except Exception:
                self.config = old
                raise
            self.recorder, self.transcriber, self.text_output = recorder, transcriber, output
            self.trigger_key = candidate.get('input', 'trigger_key')
            self.notifier.enabled = candidate.get('notifications', 'enabled')
            self.notifier.audio_feedback = candidate.get('notifications', 'audio_feedback')
            if model_changed:
                threading.Thread(target=self._preload_model, args=(transcriber,), name='model-loader', daemon=True).start()
        finally:
            if self.running:
                self.is_listening = enabled
                self._start_listener()
            self._emit_state()

    def _emit_state(self):
        if self.on_state:
            self.on_state(self.is_listening, self.is_recording, self.is_processing)

    def _emit_error(self, message):
        logger.error(message)
        if self.on_error:
            self.on_error(message)
        # Desktop notifications must not stall hotkey release handling.
        threading.Thread(target=self.notifier.error, args=(message,), name='notification', daemon=True).start()

    def _preload_model(self, transcriber):
        try:
            transcriber.load_model()
            self._emit_state()
        except Exception as exc:
            self._emit_error(f'Model load failed: {exc}')

    def start_recording(self):
        with self._lock:
            if not self.running or not self.is_listening or self.is_recording:
                return
            if self._outputting:
                self._emit_error('Wait for text to finish typing before recording again.')
                return
            if self._pending >= self.MAX_PENDING:
                self._emit_error('Transcription queue is full. Wait for processing to finish.')
                return
            try:
                self.capture_ready = False
                self.recorder.start()
                self.is_recording = True
                self._recording_done.clear()
            except Exception as exc:
                self._emit_error(f'Recording failed: {exc}')
            self._emit_state()

    def stop_recording(self):
        with self._lock:
            if not self.is_recording:
                return
            self.is_recording = False
            self._recording_done.set()
            try:
                ok = self.recorder.stop()
                if ok and self.running:
                    self._pending += 1
                    self.is_processing = True
                    self._jobs.put((self.recorder.output_file, self._generation, time.monotonic()))
                    if self._worker is None:
                        self._worker = threading.Thread(target=self._work, daemon=True)
                        self._worker.start()
                else:
                    self.recorder.discard()
            except Exception as exc:
                self.recorder.discard()
                self._emit_error(f'Recording failed: {exc}')
            self._emit_state()

    def _work(self):
        while True:
            job = self._jobs.get()
            if job is None:
                return
            audio_file, generation, queued_at = job
            threading.current_thread().name = 'dictation-' + Path(audio_file).stem.rsplit('-', 1)[-1]
            logger.info('Dictation processing started: pending=%d queue_ms=%.0f',
                        self._pending, (time.monotonic()-queued_at)*1000)
            try:
                if self.running and generation == self._generation:
                    self._process(audio_file, generation)
            except Exception as exc:
                logger.exception('Dictation processing failed')
                self._emit_error(f'Transcription failed: {exc}')
            finally:
                Path(audio_file).unlink(missing_ok=True)
                with self._lock:
                    self._pending -= 1
                    self.is_processing = self._pending > 0
                    self._emit_state()

    def _process(self, audio_file, generation):
        processed_file = audio_file
        started = time.monotonic()
        try:
            if self.config.get('audio', 'preprocess', True):
                processed_file = preprocess_audio(audio_file)
            prepared = time.monotonic()
            text = self.transcriber.transcribe_file(processed_file)
            transcribed = time.monotonic()
            if not self.running or generation != self._generation:
                return
            if not text:
                self._emit_error('No speech detected. Check the microphone in Settings → Input.')
                return
            if not self.running:
                return
            # Preserve the text in history even when typing fails.
            if self.on_transcription:
                self.on_transcription(text, transcribed - prepared)
            # A later dictation may still hold Ctrl/Alt. Wait for release before typing.
            while True:
                self._recording_done.wait()
                with self._lock:
                    if not self.running or generation != self._generation:
                        return
                    if not self.is_recording:
                        self._outputting = True
                        break
            try:
                self.text_output.type_text(text, add_space=self.config.get('output', 'add_space'),
                                           interval=self.config.get('output', 'type_interval'))
            finally:
                with self._lock:
                    self._outputting = False
            logger.info('Dictation timing: preprocess=%.3fs transcribe=%.3fs output=%.3fs total=%.3fs',
                        prepared-started, transcribed-prepared, time.monotonic()-transcribed,
                        time.monotonic()-started)
        finally:
            cleanup_processed(processed_file, audio_file)
