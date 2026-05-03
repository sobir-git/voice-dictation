import logging
import os
import threading
import time
from typing import Callable, Optional

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
    def __init__(
        self,
        config: Config,
        on_state: Optional[Callable[[bool, bool, bool], None]] = None,
        on_transcription: Optional[Callable[[str, float], None]] = None,
        on_error: Optional[Callable[[str], None]] = None,
    ):
        self.config = config
        self.on_state = on_state
        self.on_transcription = on_transcription
        self.on_error = on_error

        self.running = True
        self.is_listening = False
        self.is_recording = False
        self.is_processing = False

        self.trigger_key = self.config.get('input', 'trigger_key', default='KEY_F16')

        self.recorder = AudioRecorder(
            sample_rate=self.config.get('audio', 'sample_rate'),
            format=self.config.get('audio', 'format'),
            channels=self.config.get('audio', 'channels'),
            output_file=self.config.get('audio', 'temp_file'),
        )
        self.transcriber = self._build_transcriber()
        self.text_output = TextOutput(method=self.config.get('output', 'method'))
        self.notifier = Notifier(
            enabled=self.config.get('notifications', 'enabled'),
            audio_feedback=self.config.get('notifications', 'audio_feedback'),
        )

        self._listener: Optional[HotkeyListener] = None

    def _build_transcriber(self) -> Transcriber:
        return Transcriber(
            model_name=self.config.get('transcription', 'model'),
            compute_type=self.config.get('transcription', 'compute_type'),
            language=self.config.get('transcription', 'language'),
            beam_size=self.config.get('transcription', 'beam_size'),
            vad_filter=self.config.get('transcription', 'vad_filter'),
        )

    def _build_listener(self) -> HotkeyListener:
        return HotkeyListener(
            trigger_key=self.trigger_key,
            on_key_down=self.start_recording,
            on_key_up=self.stop_recording,
            on_error=lambda e: self._emit_error(str(e)),
            is_running=lambda: self.running,
            is_enabled=lambda: self.is_listening,
        )

    def _start_listener(self, error_prefix: str) -> bool:
        self._listener = self._build_listener()
        try:
            self._listener.validate()
        except Exception as e:
            self._emit_error(f'{error_prefix}: {e}')
            return False

        self.is_listening = True
        self._emit_state()
        self._listener.start(daemon=True)
        return True

    def start(self) -> bool:
        has_permissions, msg = DeviceDetector.check_permissions()
        if not has_permissions:
            self._emit_error(msg)
            return False

        if not self._start_listener('Failed to open input device'):
            return False

        threading.Thread(target=self._preload_model, daemon=True).start()
        return True

    def stop(self) -> None:
        self.running = False
        self.is_listening = False
        if self.is_recording:
            self.stop_recording()
        self._emit_state()

    def set_listening(self, enabled: bool) -> None:
        self.is_listening = enabled
        self._emit_state()

    def restart_listener(self) -> None:
        """Restart the hotkey listener with current config (called after key/device change)."""
        self.running = False
        self.is_listening = False
        if self.is_recording:
            self.stop_recording()
        self._emit_state()
        time.sleep(0.3)

        self.trigger_key = self.config.get('input', 'trigger_key', default='KEY_F16')
        self.running = True

        if not self._start_listener('Failed to open device'):
            return

        logger.info('Listener restarted with key %s', self.trigger_key)

    def reload_transcriber(self) -> None:
        """Reload transcriber and text output with current config settings."""
        logger.info('Reloading transcriber with new settings')
        self.transcriber = self._build_transcriber()
        self.text_output = TextOutput(method=self.config.get('output', 'method'))
        threading.Thread(target=self._preload_model, daemon=True).start()

    def _emit_state(self) -> None:
        if self.on_state:
            self.on_state(self.is_listening, self.is_recording, self.is_processing)

    def _emit_error(self, message: str) -> None:
        logger.error(message)
        try:
            self.notifier.error(message)
        except Exception:
            pass
        if self.on_error:
            self.on_error(message)

    def _preload_model(self) -> None:
        try:
            self.transcriber.load_model()
            logger.info('Model loaded')
        except Exception as e:
            logger.error('Model load error: %s', e)

    def start_recording(self) -> None:
        if self.is_recording:
            return
        self.is_recording = True
        self._emit_state()
        self.recorder.start()

    def stop_recording(self) -> None:
        if not self.is_recording:
            return
        self.is_recording = False
        self._emit_state()
        ok = self.recorder.stop()
        if ok:
            threading.Thread(target=self._process, daemon=True).start()

    def _process(self) -> None:
        audio_file = self.config.get('audio', 'temp_file')
        if not os.path.exists(audio_file):
            return

        self.is_processing = True
        self._emit_state()
        processed_file = audio_file
        try:
            if self.config.get('audio', 'preprocess', default=True):
                processed_file = preprocess_audio(audio_file)

            start = time.time()
            text = self.transcriber.transcribe_file(processed_file)
            duration = time.time() - start

            if not text:
                return

            logger.debug('Transcribed text: %r', text)
            add_space = self.config.get('output', 'add_space')
            interval = self.config.get('output', 'type_interval')
            self.text_output.type_text(text, add_space=add_space, interval=interval)

            if self.on_transcription:
                self.on_transcription(text, duration)
        except Exception as e:
            self._emit_error(f'Transcription error: {e}')
        finally:
            cleanup_processed(processed_file, audio_file)
            self.is_processing = False
            self._emit_state()
