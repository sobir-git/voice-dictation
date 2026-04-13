import logging
import os
import shutil
import subprocess

logger = logging.getLogger(__name__)


class TextOutput:
    def __init__(self, method: str = 'auto'):
        self.method = method

    def detect_method(self) -> str:
        if self.method != 'auto':
            return self.method

        on_wayland = bool(os.environ.get('WAYLAND_DISPLAY'))

        if on_wayland:
            order = [
                ('wtype', 'wtype'),
                ('dotool', 'dotool'),
                ('ydotool', 'ydotool'),
                ('xdotool', 'xdotool'),
                ('xclip', 'xclip'),
            ]
        else:
            order = [
                ('dotool', 'dotool'),
                ('ydotool', 'ydotool'),
                ('xdotool', 'xdotool'),
                ('wtype', 'wtype'),
                ('xclip', 'xclip'),
            ]

        for m, bin_name in order:
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
                if os.environ.get('WAYLAND_DISPLAY'):
                    import time
                    time.sleep(0.3)
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
