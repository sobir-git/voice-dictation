import logging
import os
import shutil
import subprocess
import time

logger = logging.getLogger(__name__)


class TextOutput:
    def __init__(self, method: str = 'auto'):
        self._resolved_method = self._resolve_method(method)
        self._ydotoold_socket_path = self._ydotoold_socket()
        label = 'auto-detected' if method == 'auto' else 'configured'
        logger.info("Text output method: %s (%s)", self._resolved_method, label)

    def _resolve_method(self, method: str) -> str:
        if method != 'auto':
            return method

        on_wayland = bool(os.environ.get('WAYLAND_DISPLAY'))

        if on_wayland:
            order = ['wtype', 'dotool', 'ydotool', 'xdotool', 'xclip']
        else:
            order = ['dotool', 'ydotool', 'xdotool', 'wtype', 'xclip']

        for m in order:
            if shutil.which(m) and self._probe(m):
                return m
        return 'none'

    def _probe(self, method: str) -> bool:
        """Return True if the method is actually functional (not just installed).

        wtype   — requires zwp_virtual_keyboard_manager_v1; GNOME Wayland lacks it.
        ydotool — requires the ydotoold daemon socket to be present; raw uinput
                  fallback drops keystrokes.
        """
        try:
            if method == 'wtype':
                r = subprocess.run(['wtype', ''], capture_output=True, timeout=2)
                return r.returncode == 0
            if method == 'ydotool':
                return self._ydotoold_socket() is not None
            return True
        except Exception:
            return False

    def _ydotoold_socket(self) -> str | None:
        """Return the ydotoold socket path if the daemon is running, else None."""
        candidates = [
            os.environ.get('YDOTOOL_SOCKET', ''),
            f'/run/user/{os.getuid()}/.ydotool_socket',
            '/tmp/.ydotool_socket',
        ]
        for path in candidates:
            if path and os.path.exists(path):
                return path
        return None

    def type_text(self, text: str, add_space: bool = True, interval: float = 0.0) -> None:
        method = self._resolved_method
        if method == 'none':
            logger.warning("No working text output method available; text dropped: %r", text)
            return

        out = text + (' ' if add_space else '')

        if method in ('xdotool', 'ydotool', 'dotool', 'wtype'):
            self._type_with_tool(method, out, interval)
        elif method == 'xclip':
            self._copy_clipboard_xclip(out)

    def _type_with_tool(self, method: str, text: str, interval: float) -> None:
        try:
            if method == 'xdotool':
                subprocess.run(
                    ['xdotool', 'type', '--delay', str(int(interval * 1000)), text],
                    check=False,
                )
            elif method == 'ydotool':
                if os.environ.get('WAYLAND_DISPLAY'):
                    time.sleep(0.3)
                key_delay = int(interval * 1000)
                env = None
                if self._ydotoold_socket_path:
                    env = os.environ.copy()
                    env['YDOTOOL_SOCKET'] = self._ydotoold_socket_path
                subprocess.run(
                    ['ydotool', 'type', f'--key-delay={key_delay}', text],
                    check=False, env=env,
                )
            elif method == 'dotool':
                p = subprocess.Popen(['dotool'], stdin=subprocess.PIPE)
                assert p.stdin is not None
                p.stdin.write(f'type {text}\n'.encode('utf-8'))
                p.stdin.close()
            elif method == 'wtype':
                subprocess.run(['wtype', text], check=False)
        except Exception as e:
            logger.error("Failed to output text via %s: %s", method, e)

    def _copy_clipboard_xclip(self, text: str) -> None:
        try:
            subprocess.run(
                ['xclip', '-selection', 'clipboard'],
                input=text.encode('utf-8'),
                check=False,
            )
        except Exception as e:
            logger.error("Failed to copy to clipboard: %s", e)
