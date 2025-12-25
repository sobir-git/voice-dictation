import logging
import shutil
import subprocess
from typing import Optional

logger = logging.getLogger(__name__)


class TextOutput:
    def __init__(self, method: str = 'auto'):
        self.method = method

    def detect_method(self) -> str:
        if self.method != 'auto':
            return self.method

        for m, bin_name in (
            ('dotool', 'dotool'),
            ('ydotool', 'ydotool'),
            ('xdotool', 'xdotool'),
            ('wtype', 'wtype'),
            ('xclip', 'xclip'),
        ):
            if shutil.which(bin_name):
                return m
        return 'none'

    def type_text(self, text: str, add_space: bool = True, interval: float = 0.0) -> None:
        method = self.detect_method()
        if method == 'none':
            logger.info("No output method available; skipping typing")
            return

        out = text + (' ' if add_space else '')

        if method in ('xdotool', 'ydotool', 'dotool', 'wtype'):
            self._type_with_tool(method, out, interval)
            return

        if method == 'xclip':
            self._copy_clipboard_xclip(out)
            return

    def _type_with_tool(self, method: str, text: str, interval: float) -> None:
        try:
            if method == 'xdotool':
                subprocess.run(['xdotool', 'type', '--delay', str(int(interval * 1000)), text], check=False)
            elif method == 'ydotool':
                subprocess.run(['ydotool', 'type', text], check=False)
            elif method == 'dotool':
                p = subprocess.Popen(['dotool'], stdin=subprocess.PIPE)
                assert p.stdin is not None
                p.stdin.write(f'type {text}\n'.encode('utf-8'))
                p.stdin.close()
            elif method == 'wtype':
                subprocess.run(['wtype', text], check=False)
        except Exception as e:
            logger.error("Failed to output text: %s", e)

    def _copy_clipboard_xclip(self, text: str) -> None:
        try:
            subprocess.run(['xclip', '-selection', 'clipboard'], input=text.encode('utf-8'), check=False)
        except Exception as e:
            logger.error("Failed to copy to clipboard: %s", e)
