import glob
import os
from typing import Tuple

from evdev import InputDevice
from evdev import ecodes


class DeviceDetector:
    @staticmethod
    def check_permissions() -> Tuple[bool, str]:
        paths = glob.glob('/dev/input/event*')
        if not paths:
            return False, 'No /dev/input/event* devices found'

        for p in paths:
            try:
                with open(p, 'rb'):
                    pass
                return True, 'OK'
            except PermissionError:
                continue
            except Exception:
                continue

        return False, 'Permission denied reading /dev/input/event*. Add user to input group or install udev rules.'

    @staticmethod
    def detect_trigger_key_device(trigger_key: str) -> str:
        key_code = ecodes.ecodes.get(trigger_key)
        if not key_code:
            return ''

        for path in glob.glob('/dev/input/event*'):
            try:
                dev = InputDevice(path)
                caps = dev.capabilities(verbose=False)
                keys = caps.get(ecodes.ecodes.get('EV_KEY', 1), [])
                if key_code in keys:
                    return path
            except Exception:
                continue

        return ''
