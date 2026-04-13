#!/usr/bin/env python3

import argparse
import logging
import signal
import sys

from speech_to_text.core.app_logging import setup_logging
from speech_to_text.core.config import Config
from speech_to_text.core.controller import SpeechToTextController
from speech_to_text.core.device_detector import DeviceDetector
from speech_to_text.core.text_output import TextOutput


def main(argv=None):
    parser = argparse.ArgumentParser(description='Speech-to-Text listener (diagnostic)')
    parser.add_argument('--list-devices', action='store_true')
    parser.add_argument('--detect-output', action='store_true')
    args = parser.parse_args(argv)

    config = Config()
    setup_logging(config)
    logger = logging.getLogger(__name__)

    if args.list_devices:
        from glob import glob
        from evdev import InputDevice
        for p in glob('/dev/input/event*'):
            try:
                dev = InputDevice(p)
                print(f"{p}: {dev.name}")
            except Exception:
                continue
        return 0

    if args.detect_output:
        print(TextOutput(method='auto')._resolved_method)
        return 0

    ok, msg = DeviceDetector.check_permissions()
    if not ok:
        logger.error(msg)
        return 2

    controller = SpeechToTextController(config=config)

    def handle(_sig, _frame):
        controller.stop()
        sys.exit(0)

    signal.signal(signal.SIGINT, handle)
    signal.signal(signal.SIGTERM, handle)

    if not controller.start():
        return 1

    logger.info('Listening... press Ctrl+C to stop')
    signal.pause()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
