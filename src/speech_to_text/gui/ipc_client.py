import json
import os
import socket
import threading
import time

from speech_to_text.gui.gtk import GLib

SOCKET_PATH = '/tmp/stt_daemon.sock'


class DaemonClient:
    """Persistent IPC client for the tray app."""

    def __init__(self, socket_path: str, on_message):
        self.socket_path = socket_path
        self.on_message = on_message
        self._running = True
        self._sock: socket.socket | None = None
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._loop, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._running = False
        self._close()

    def send(self, msg: dict) -> None:
        data = (json.dumps(msg) + '\n').encode()
        with self._lock:
            if self._sock:
                try:
                    self._sock.sendall(data)
                except Exception:
                    self._close()

    def _close(self) -> None:
        with self._lock:
            if self._sock:
                try:
                    self._sock.close()
                except Exception:
                    pass
                self._sock = None

    def _loop(self) -> None:
        while self._running:
            try:
                self._connect_and_read()
            except Exception:
                pass
            if self._running:
                GLib.idle_add(self.on_message, {'type': 'disconnected'})
                time.sleep(2)

    def _connect_and_read(self) -> None:
        while self._running:
            if os.path.exists(self.socket_path):
                break
            time.sleep(1)

        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(5)
        sock.connect(self.socket_path)
        sock.settimeout(None)
        with self._lock:
            self._sock = sock

        GLib.idle_add(self.on_message, {'type': 'connected'})

        buffer = b''
        while self._running:
            chunk = sock.recv(4096)
            if not chunk:
                break
            buffer += chunk
            while b'\n' in buffer:
                line, buffer = buffer.split(b'\n', 1)
                line = line.strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                GLib.idle_add(self.on_message, msg)

        self._close()
