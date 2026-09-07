import json
import logging
import socket
import threading

from speech_to_text.core.runtime import SOCKET_PATH, ensure_daemon

logger = logging.getLogger(__name__)


class DaemonClient:
    """Reconnect after daemon restarts; callbacks run on the reader thread."""

    def __init__(self, socket_path, on_message, reconnect=None):
        self.socket_path = socket_path
        self.on_message = on_message
        self.reconnect = reconnect
        self._stop = threading.Event()
        self._sock = None
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._loop, name='ipc-client', daemon=True)

    def start(self):
        self._thread.start()

    def stop(self):
        self._stop.set()
        self._close()

    def send(self, msg):
        data = (json.dumps(msg) + '\n').encode()
        failed = False
        with self._lock:
            sock = self._sock
            if sock is None:
                return False
            try:
                sock.sendall(data)
            except OSError:
                failed = True
        if failed:
            logger.warning('IPC send failed', exc_info=False)
            self._close()
        return not failed

    def _close(self):
        with self._lock:
            sock, self._sock = self._sock, None
        if sock:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            sock.close()

    def _loop(self):
        while not self._stop.is_set():
            try:
                self._connect_and_read()
            except (OSError, ValueError) as exc:
                logger.debug('IPC reconnect needed: %s', exc)
            finally:
                self._close()
            if self._stop.is_set():
                break
            self.on_message( {'type': 'disconnected'})
            if self.reconnect:
                try:
                    self.reconnect()
                except Exception:
                    logger.warning('Daemon recovery failed', exc_info=True)
            self._stop.wait(2)

    def _connect_and_read(self):
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.settimeout(0.5)
            sock.connect(self.socket_path)
            with self._lock:
                if self._stop.is_set():
                    return
                self._sock = sock
            logger.info('Connected to daemon at %s', self.socket_path)
            self.on_message( {'type': 'connected'})
            buffer = b''
            while not self._stop.is_set():
                try:
                    chunk = sock.recv(4096)
                except socket.timeout:
                    continue
                if not chunk:
                    return
                buffer += chunk
                if len(buffer) > 1024 * 1024:
                    raise ValueError('IPC message exceeds 1 MiB')
                while b'\n' in buffer:
                    line, buffer = buffer.split(b'\n', 1)
                    try:
                        msg = json.loads(line)
                        if isinstance(msg, dict):
                            self.on_message( msg)
                    except (ValueError, UnicodeError):
                        logger.warning('Invalid message from daemon')
        finally:
            sock.close()
