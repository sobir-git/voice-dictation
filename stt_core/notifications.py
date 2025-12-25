import logging
import os
import subprocess

logger = logging.getLogger(__name__)


class Notifier:
    def __init__(self, enabled: bool = True, audio_feedback: bool = True):
        self.enabled = enabled
        self.audio_feedback = audio_feedback
        self._notify_send_available = self._check_notify_send()
        self._paplay_available = self._check_paplay()

    def _check_notify_send(self) -> bool:
        try:
            subprocess.run(['which', 'notify-send'], capture_output=True, check=True)
            return True
        except subprocess.CalledProcessError:
            logger.debug("notify-send not available")
            return False

    def _check_paplay(self) -> bool:
        try:
            subprocess.run(['which', 'paplay'], capture_output=True, check=True)
            return True
        except subprocess.CalledProcessError:
            logger.debug("paplay not available")
            return False

    def notify(
        self,
        title: str,
        message: str,
        urgency: str = 'normal',
        timeout: int = 3000,
    ):
        if not self.enabled or not self._notify_send_available:
            return

        try:
            subprocess.run(
                ['notify-send', '-u', urgency, '-t', str(timeout), title, message],
                timeout=5,
                check=False,
            )
        except Exception as e:
            logger.debug("Failed to send notification: %s", e)

    def play_sound(self, sound_type: str = 'start'):
        if not self.audio_feedback or not self._paplay_available:
            return

        sound_paths = {
            'start': '/usr/share/sounds/freedesktop/stereo/audio-volume-change.oga',
            'stop': '/usr/share/sounds/freedesktop/stereo/complete.oga',
            'error': '/usr/share/sounds/freedesktop/stereo/dialog-error.oga',
        }

        sound_path = sound_paths.get(sound_type)
        if not sound_path or not os.path.exists(sound_path):
            return

        try:
            subprocess.Popen(
                ['paplay', sound_path],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except Exception as e:
            logger.debug("Failed to play sound: %s", e)

    def error(self, message: str):
        self.notify('Speech-to-Text Error', message, urgency='critical', timeout=5000)
        self.play_sound('error')
