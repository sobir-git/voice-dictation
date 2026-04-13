#!/usr/bin/env python3

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / 'src'))

from speech_to_text.listener import main


if __name__ == '__main__':
    raise SystemExit(main())
