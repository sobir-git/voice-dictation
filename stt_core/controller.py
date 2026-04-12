import logging
import os
import threading
import time
from typing import Callable, Optional

from stt_core.audio_preprocessor import cleanup_processed, preprocess_audio
from stt_core.audio_recorder import AudioRecorder
from stt_core.config import Config
from stt_core.device_detector import DeviceDetector
from stt_core.hotkey_listener import HotkeyListener
from stt_core.notifications import Notifier
from stt_core.text_output import TextOutput
from stt_core.transcriber import Transcriber

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
        self.device_path = self._detect_device()

        self.recorder = AudioRecorder(
            sample_rate=self.config.get('audio', 'sample_rate'),
            format=self.config.get('audio', 'format'),
            channels=self.config.get('audio', 'channels'),
            output_file=self.config.get('audio', 'temp_file'),
        )
        self.transcriber = Transcriber(
            model_name=self.config.get('transcription', 'model'),
            compute_type=self.config.get('transcription', 'compute_type'),
            language=self.config.get('transcription', 'language'),
            beam_size=self.config.get('transcription', 'beam_size'),
            vad_filter=self.config.get('transcription', 'vad_filter'),
        )
        self.text_output = TextOutput(method=self.config.get('output', 'method'))
        self.notifier = Notifier(
            enabled=self.config.get('notifications', 'enabled'),
            audio_feedback=self.config.get('notifications', 'audio_feedback'),
        )

        self._listener: Optional[HotkeyListener] = None

    def _detect_device(self) -> str:
        path = self.config.get('input', 'device_path')
        if path and os.path.exists(path):
            return path
        key = self.config.get('input', 'trigger_key', default='KEY_F16')
        return DeviceDetector.detect_trigger_key_device(key)

    def start(self) -> bool:
        if not self.device_path:
            self._emit_error('No input device found')
            return False

        has_permissions, msg = DeviceDetector.check_permissions()
        if not has_permissions:
            self._emit_error(msg)
            return False

        self._listener = HotkeyListener(
            device_path=self.device_path,
            trigger_key=self.trigger_key,
            on_key_down=self.start_recording,
            on_key_up=self.stop_recording,
            on_error=lambda e: self._emit_error(str(e)),
            is_running=lambda: self.running,
            is_enabled=lambda: self.is_listening,
        )
        try:
            self._listener.validate()
        except Exception as e:
            self._emit_error(f'Failed to open input device {self.device_path}: {e}')
            return False

        self.is_listening = True
        self._emit_state()
        self._listener.start(daemon=True)
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
        # Stop current listener
        self.running = False
        self.is_listening = False
        if self.is_recording:
            self.stop_recording()
        self._emit_state()
        time.sleep(0.3)   # give the evdev read-loop thread time to exit

        # Re-read config
        self.trigger_key = self.config.get('input', 'trigger_key', default='KEY_F16')
        self.device_path = self._detect_device()
        self.running = True

        if not self.device_path:
            self._emit_error('No input device found for new hotkey')
            return

        self._listener = HotkeyListener(
            device_path=self.device_path,
            trigger_key=self.trigger_key,
            on_key_down=self.start_recording,
            on_key_up=self.stop_recording,
            on_error=lambda e: self._emit_error(str(e)),
            is_running=lambda: self.running,
            is_enabled=lambda: self.is_listening,
        )
        try:
            self._listener.validate()
        except Exception as e:
            self._emit_error(f'Failed to open device {self.device_path}: {e}')
            return

        self.is_listening = True
        self._emit_state()
        self._listener.start(daemon=True)
        logger.info('Listener restarted on %s with key %s', self.device_path, self.trigger_key)

    def reload_transcriber(self) -> None:
        """Reload transcriber with current config settings."""
        logger.info('Reloading transcriber with new settings')
        self.transcriber = Transcriber(
            model_name=self.config.get('transcription', 'model'),
            compute_type=self.config.get('transcription', 'compute_type'),
            language=self.config.get('transcription', 'language'),
            beam_size=self.config.get('transcription', 'beam_size'),
            vad_filter=self.config.get('transcription', 'vad_filter'),
        )
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
