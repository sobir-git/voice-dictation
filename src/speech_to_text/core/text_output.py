import logging
import os
import shutil
import subprocess
import time

logger = logging.getLogger(__name__)


class TextOutput:
    def __init__(self, method: str = 'auto'):
        self.method = method
        self._resolved_method = self._resolve_method(method)
        self._ydotoold_socket_path = self._ydotoold_socket()
        label = 'auto-detected' if method == 'auto' else 'configured'
        logger.info("Text output method: %s (%s)", self._resolved_method, label)

    def _resolve_method(self, method: str) -> str:
        if method != 'auto':
            return method

        on_wayland = bool(os.environ.get('WAYLAND_DISPLAY'))

        if on_wayland:
            order = ['wtype', 'dotool', 'ydotool']
        else:
            order = ['xdotool', 'dotool', 'ydotool', 'wtype']

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
            if method == 'xdotool':
                return subprocess.run(['xdotool', 'getdisplaygeometry'], capture_output=True, timeout=2).returncode == 0
            if method == 'dotool':
                return os.access('/dev/uinput', os.W_OK)
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
        if self.method == 'none':
            return  # Explicit history-only mode.
        if self._resolved_method == 'none':
            self._resolved_method = self._resolve_method(self.method)
        method = self._resolved_method
        if method == 'none':
            raise RuntimeError('No working text output tool. Text is saved in History.')
        out = text + (' ' if add_space else '')
        delay = str(int(interval * 1000))
        env = None
        payload = None
        if method == 'xdotool':
            # X11 temporarily maps non-ASCII keysyms. Zero delay can restore the
            # mapping before the application reads the event.
            if not out.isascii():
                delay = str(max(12, int(delay)))
            cmd = ['xdotool', 'type', '--clearmodifiers', '--delay', delay, '--file', '-']
            payload = out.encode()
        elif method == 'ydotool':
            path = self._ydotoold_socket()
            if not path:
                raise RuntimeError('ydotoold is unavailable. Text is saved in History.')
            env = {**os.environ, 'YDOTOOL_SOCKET': path}
            cmd = ['ydotool', 'type', f'--key-delay={delay}', '--file', '-']
            payload = out.encode()
        elif method == 'wtype':
            cmd = ['wtype', '-d', delay, '--', out]
        elif method == 'dotool':
            cmd = ['dotool']
            # Each line is a type command, never executable dotool instructions.
            lines = out.split('\n')
            payload = ('\nkey enter\n'.join('type ' + line for line in lines) + '\n').encode()
        elif method == 'xclip':
            cmd = ['xclip', '-selection', 'clipboard']
            payload = out.encode()
        else:
            raise ValueError(f'Unknown text output method: {method}')
        started = time.monotonic()
        try:
            subprocess.run(cmd, input=payload, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                           timeout=max(10, len(out) * interval * 2 + 5), env=env, check=True)
        except subprocess.CalledProcessError as exc:
            # Do not log the command: it contains the user's dictation.
            logger.error('Text output failed: method=%s exit=%s', method, exc.returncode)
            raise RuntimeError(f'{method} exited with status {exc.returncode}. Text is saved in History.') from None
        except (OSError, subprocess.TimeoutExpired) as exc:
            logger.error('Text output failed: method=%s error=%s', method, type(exc).__name__)
            raise RuntimeError(f'{method} could not deliver text. Text is saved in History.') from None
        logger.debug('Text output delivered: method=%s characters=%d elapsed=%.3fs', method, len(out), time.monotonic()-started)
