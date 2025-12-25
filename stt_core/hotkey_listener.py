import logging
import threading
from typing import Callable, Optional

from evdev import InputDevice, categorize
from evdev import ecodes

logger = logging.getLogger(__name__)


class HotkeyListener:
    def __init__(
        self,
        device_path: str,
        trigger_key: str,
        on_key_down: Callable[[], None],
        on_key_up: Callable[[], None],
        on_error: Callable[[Exception], None],
        is_running: Callable[[], bool],
        is_enabled: Callable[[], bool],
    ):
        self.device_path = device_path
        self.trigger_key = trigger_key
        self.on_key_down = on_key_down
        self.on_key_up = on_key_up
        self.on_error = on_error
        self.is_running = is_running
        self.is_enabled = is_enabled

        self._thread: Optional[threading.Thread] = None

    def validate(self) -> None:
        dev = InputDevice(self.device_path)
        dev.close()

    def start(self, daemon: bool = True) -> None:
        if self._thread and self._thread.is_alive():
            return

        self._thread = threading.Thread(target=self._run, daemon=daemon)
        self._thread.start()

    def _run(self) -> None:
        try:
            dev = InputDevice(self.device_path)
        except Exception as e:
            self.on_error(e)
            return

        key_code = ecodes.ecodes.get(self.trigger_key)
        ev_key = ecodes.ecodes.get('EV_KEY', 1)

        if not key_code:
            self.on_error(ValueError(f'Unknown key: {self.trigger_key}'))
            return

        try:
            for event in dev.read_loop():
                if not self.is_running():
                    break
                if not self.is_enabled():
                    continue
                if event.type != ev_key:
                    continue
                e = categorize(event)
                if e.scancode != key_code:
                    continue
                if e.keystate == e.key_down:
                    self.on_key_down()
                elif e.keystate == e.key_up:
                    self.on_key_up()
        except Exception as e:
            logger.error("Hotkey listener error: %s", e)
            self.on_error(e)
        finally:
            try:
                dev.close()
            except Exception:
                pass
