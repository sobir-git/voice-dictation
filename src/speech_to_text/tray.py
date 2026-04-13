#!/usr/bin/env python3

import signal
import sys

from speech_to_text.gui.tray_app import STTTrayApp


def main():
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    app = STTTrayApp()
    sys.exit(app.run(sys.argv))


if __name__ == '__main__':
    main()
