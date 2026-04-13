#!/usr/bin/env python3
"""
Speech-to-Text Daemon — headless, no GTK.

Responsibilities:
  - evdev hotkey listener (hold-to-record)
  - arecord audio capture
  - faster-whisper transcription
  - ydotool text output
  - Unix socket IPC server for tray / external clients

Socket protocol (newline-delimited JSON):
  Server → clients:  {"type": "state", "listening": bool, "recording": bool, "processing": bool}
                     {"type": "transcription", "text": "...", "duration": 1.23}
                     {"type": "error", "message": "..."}
  Client → server:   {"cmd": "toggle_listening"}
                     {"cmd": "set_listening", "value": true|false}
                     {"cmd": "reload_config"}
                     {"cmd": "quit"}
"""

import json
import logging
import os
import signal
import socket
import sys
import threading

from speech_to_text.core.app_logging import setup_logging
from speech_to_text.core.config import Config
from speech_to_text.core.controller import SpeechToTextController

SOCKET_PATH = '/tmp/stt_daemon.sock'
LOG = logging.getLogger('stt_daemon')


# ── IPC server ──────────────────────────────────────────────────────────────

class IPCServer:
    """Broadcast state updates to all connected tray clients; accept commands."""

    def __init__(self, socket_path: str, controller: 'SpeechToTextController'):
        self.socket_path = socket_path
        self.controller = controller
        self._clients: list[socket.socket] = []
        self._lock = threading.Lock()
        self._server: socket.socket | None = None
        self._running = True

    def start(self) -> None:
        try:
            os.unlink(self.socket_path)
        except FileNotFoundError:
            pass
        self._server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._server.bind(self.socket_path)
        os.chmod(self.socket_path, 0o600)
        self._server.listen(8)
        self._server.settimeout(1.0)
        threading.Thread(target=self._accept_loop, daemon=True).start()
        LOG.info('IPC socket listening at %s', self.socket_path)

    def stop(self) -> None:
        self._running = False
        if self._server:
            try:
                self._server.close()
            except Exception:
                pass
        try:
            os.unlink(self.socket_path)
        except Exception:
            pass

    def broadcast(self, msg: dict) -> None:
        data = (json.dumps(msg) + '\n').encode()
        dead = []
        with self._lock:
            for c in self._clients:
                try:
                    c.sendall(data)
                except Exception:
                    dead.append(c)
            for c in dead:
                self._clients.remove(c)
                try:
                    c.close()
                except Exception:
                    pass

    def _accept_loop(self) -> None:
        while self._running:
            try:
                conn, _ = self._server.accept()
                threading.Thread(target=self._handle, args=(conn,), daemon=True).start()
            except socket.timeout:
                continue
            except Exception:
                if self._running:
                    LOG.exception('IPC accept error')
                break

    def _handle(self, conn: socket.socket) -> None:
        with self._lock:
            self._clients.append(conn)
        # Send current state immediately on connect
        self.broadcast(self._state_msg())
        buf = b''
        try:
            while self._running:
                chunk = conn.recv(4096)
                if not chunk:
                    break
                buf += chunk
                while b'\n' in buf:
                    line, buf = buf.split(b'\n', 1)
                    self._dispatch(line.strip())
        except Exception:
            pass
        finally:
            with self._lock:
                if conn in self._clients:
                    self._clients.remove(conn)
            try:
                conn.close()
            except Exception:
                pass

    def _state_msg(self) -> dict:
        c = self.controller
        return {
            'type': 'state',
            'listening': c.is_listening,
            'recording': c.is_recording,
            'processing': c.is_processing,
        }

    def _dispatch(self, line: bytes) -> None:
        if not line:
            return
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            return

        cmd = msg.get('cmd')
        if cmd == 'toggle_listening':
            self.controller.set_listening(not self.controller.is_listening)
            return
        if cmd == 'set_listening':
            self.controller.set_listening(bool(msg.get('value', True)))
            return
        if cmd == 'reload_config':
            self.controller.config.load()
            self.controller.reload_transcriber()
            self.controller.restart_listener()
            return
        if cmd == 'quit':
            LOG.info('Quit command received via IPC')
            os.kill(os.getpid(), signal.SIGTERM)


# ── Daemon ───────────────────────────────────────────────────────────────────

class STTDaemon:
    def __init__(self):
        self.config = Config()
        setup_logging(self.config)

        self._stop_event = threading.Event()

        self.controller = SpeechToTextController(
            config=self.config,
            on_state=self._on_state,
            on_transcription=self._on_transcription,
            on_error=self._on_error,
        )

        self.ipc = IPCServer(SOCKET_PATH, self.controller)

    def _on_state(self, listening: bool, recording: bool, processing: bool) -> None:
        self.ipc.broadcast(
            {
            'type': 'state',
            'listening': listening,
            'recording': recording,
            'processing': processing,
            }
        )

    def _on_transcription(self, text: str, duration: float) -> None:
        self.ipc.broadcast({'type': 'transcription', 'text': text, 'duration': duration})

    def _on_error(self, message: str) -> None:
        self.ipc.broadcast({'type': 'error', 'message': message})

    def run(self) -> None:
        signal.signal(signal.SIGTERM, self._handle_signal)
        signal.signal(signal.SIGINT, self._handle_signal)

        self.ipc.start()

        ok = self.controller.start()
        if not ok:
            LOG.error('Controller failed to start — daemon exiting')
            self.ipc.stop()
            sys.exit(1)

        LOG.info('STT daemon running (pid %d)', os.getpid())
        self._stop_event.wait()

        LOG.info('STT daemon shutting down')
        self.controller.stop()
        self.ipc.stop()

    def _handle_signal(self, sig, frame) -> None:
        LOG.info('Signal %d received', sig)
        self._stop_event.set()


def main():
    daemon = STTDaemon()
    daemon.run()


if __name__ == '__main__':
    main()
