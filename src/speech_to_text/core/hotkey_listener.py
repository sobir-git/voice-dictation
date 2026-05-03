import glob
import logging
import selectors
import threading
import time
from typing import Callable, Optional

from evdev import InputDevice
from evdev import ecodes

logger = logging.getLogger(__name__)


class HotkeyListener:
    def __init__(
        self,
        trigger_key: str,
        on_key_down: Callable[[], None],
        on_key_up: Callable[[], None],
        on_error: Callable[[Exception], None],
        is_running: Callable[[], bool],
        is_enabled: Callable[[], bool],
    ):
        self.trigger_key = trigger_key
        self.on_key_down = on_key_down
        self.on_key_up = on_key_up
        self.on_error = on_error
        self.is_running = is_running
        self.is_enabled = is_enabled

        self._thread: Optional[threading.Thread] = None
        self._pressed_sources: set[str] = set()

    def validate(self) -> None:
        # Startup permissions are validated separately by DeviceDetector.
        return

    def start(self, daemon: bool = True) -> None:
        if self._thread and self._thread.is_alive():
            return

        self._thread = threading.Thread(target=self._run, daemon=daemon)
        self._thread.start()

    def _list_devices(self, key_code: int) -> list[str]:
        paths = []
        for path in sorted(glob.glob('/dev/input/event*')):
            try:
                dev = InputDevice(path)
                caps = dev.capabilities(verbose=False)
                keys = caps.get(ecodes.ecodes.get('EV_KEY', 1), [])
                if key_code in keys:
                    paths.append(path)
                dev.close()
            except Exception:
                continue
        return paths

    def _open_devices(self, selector: selectors.BaseSelector, devices: dict[str, InputDevice], key_code: int) -> None:
        current_paths = set(self._list_devices(key_code))

        # Drop disappeared devices.
        for path in list(devices):
            if path not in current_paths:
                dev = devices.pop(path)
                try:
                    selector.unregister(dev)
                except Exception:
                    pass
                try:
                    dev.close()
                except Exception:
                    pass

        # Add new devices.
        for path in current_paths:
            if path in devices:
                continue
            try:
                dev = InputDevice(path)
                selector.register(dev, selectors.EVENT_READ)
                devices[path] = dev
            except Exception:
                continue

    def _run(self) -> None:
        key_code = ecodes.ecodes.get(self.trigger_key)
        ev_key = ecodes.ecodes.get('EV_KEY', 1)

        if not key_code:
            self.on_error(ValueError(f'Unknown key: {self.trigger_key}'))
            return

        selector = selectors.DefaultSelector()
        devices: dict[str, InputDevice] = {}
        last_rescan = 0.0

        try:
            while self.is_running():
                now = time.monotonic()
                if now - last_rescan >= 1.0:
                    self._open_devices(selector, devices, key_code)
                    last_rescan = now

                if not devices:
                    time.sleep(1.0)
                    continue

                for key, _ in selector.select(timeout=0.5):
                    if not self.is_running():
                        break
                    if not self.is_enabled():
                        continue

                    device = key.fileobj
                    try:
                        for event in device.read():
                            if event.type != ev_key:
                                continue
                            if event.code != key_code:
                                continue
                            source = getattr(device, 'path', '')
                            if event.value == 1:
                                was_inactive = not self._pressed_sources
                                self._pressed_sources.add(source)
                                if was_inactive:
                                    self.on_key_down()
                            elif event.value == 0:
                                self._pressed_sources.discard(source)
                                if not self._pressed_sources:
                                    self.on_key_up()
                    except Exception as e:
                        logger.warning('Hotkey listener device error on %s: %s', getattr(device, 'path', '?'), e)
                        path = getattr(device, 'path', None)
                        if path and path in devices:
                            was_pressed = path in self._pressed_sources
                            self._pressed_sources.discard(path)
                            if was_pressed and not self._pressed_sources:
                                self.on_key_up()
                            try:
                                selector.unregister(device)
                            except Exception:
                                pass
                            try:
                                device.close()
                            except Exception:
                                pass
                            devices.pop(path, None)
                if self.is_running():
                    time.sleep(0.05)
        except Exception as e:
            logger.error('Hotkey listener error: %s', e)
            if self.is_running():
                self.on_error(e)
        finally:
            if self._pressed_sources:
                self._pressed_sources.clear()
            for dev in list(devices.values()):
                try:
                    selector.unregister(dev)
                except Exception:
                    pass
                try:
                    dev.close()
                except Exception:
                    pass
            try:
                selector.close()
            except Exception:
                pass
