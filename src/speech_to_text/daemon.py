#!/usr/bin/env python3
"""Headless dictation service with bounded, newline-delimited JSON IPC."""
import json
import logging
import os
import signal
import socket
import threading

from speech_to_text.core.app_logging import setup_logging
from speech_to_text.core.config import Config
from speech_to_text.core.controller import SpeechToTextController
from speech_to_text.core.history import HistoryManager
from speech_to_text.core.runtime import SOCKET_PATH, InstanceLock

LOG = logging.getLogger(__name__)


class IPCServer:
    def __init__(self, socket_path, controller):
        self.socket_path = socket_path
        self.controller = controller
        self._clients = []
        self._lock = threading.Lock()
        self._commands = threading.Lock()
        self._server = None
        self._running = False
        self._instance = None
        self.last_error = ''

    def start(self):
        self._instance = InstanceLock(self.socket_path)
        try:
            if os.path.exists(self.socket_path):
                os.unlink(self.socket_path)
            self._server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self._server.bind(self.socket_path)
            os.chmod(self.socket_path, 0o600)
            self._server.listen(8)
            self._server.settimeout(0.5)
            self._running = True
            threading.Thread(target=self._accept_loop, name='ipc-accept', daemon=True).start()
            LOG.info('IPC listening at %s', self.socket_path)
        except Exception:
            self.stop()
            raise

    def stop(self):
        self._running = False
        if self._server:
            self._server.close()
        with self._lock:
            for client in self._clients:
                client.close()
            self._clients.clear()
        if self._instance:
            try:
                os.unlink(self.socket_path)
            except FileNotFoundError:
                pass
            self._instance.close()
            self._instance = None

    def broadcast(self, msg):
        if msg.get('type') == 'error':
            self.last_error = msg['message']
        data = (json.dumps(msg) + '\n').encode()
        # Each client has a short timeout; a stalled tray cannot indefinitely block recording.
        with self._lock:
            for client in list(self._clients):
                try:
                    client.sendall(data)
                except OSError:
                    self._clients.remove(client)
                    client.close()

    def _accept_loop(self):
        while self._running:
            try:
                conn, _ = self._server.accept()
                conn.settimeout(0.1)
                with self._lock:
                    if len(self._clients) >= 8:
                        conn.close()
                        continue
                    try:
                        conn.sendall((json.dumps(self._state_msg()) + '\n').encode())
                    except OSError:
                        conn.close()
                        continue
                    self._clients.append(conn)
                threading.Thread(target=self._handle, args=(conn,), name='ipc-reader', daemon=True).start()
            except socket.timeout:
                continue
            except OSError:
                if self._running:
                    LOG.exception('IPC accept failed')

    def _handle(self, conn):
        buf = b''
        try:
            while self._running:
                try:
                    chunk = conn.recv(4096)
                except socket.timeout:
                    continue
                if not chunk:
                    break
                buf += chunk
                if len(buf) > 65536:
                    LOG.warning('Dropping IPC client: command exceeds 64 KiB')
                    break
                while b'\n' in buf:
                    line, buf = buf.split(b'\n', 1)
                    self._dispatch(line)
        except OSError:
            pass
        finally:
            with self._lock:
                if conn in self._clients:
                    self._clients.remove(conn)
            conn.close()

    def _state_msg(self):
        c = self.controller
        return {'type': 'state', 'listening': c.is_listening, 'recording': c.is_recording,
                'processing': c.is_processing, 'pending': c._pending,
                'capture_ready': c.capture_ready, 'log_level': c.config.get('logging', 'level'),
                'model_ready': c.transcriber.model is not None,
                'output_method': c.text_output._resolved_method, 'last_error': self.last_error,
                'hotkey': c.trigger_key, 'microphone': c.config.get('audio', 'pipewire_node') or c.recorder.device}

    def _dispatch(self, line):
        try:
            msg = json.loads(line)
            if not isinstance(msg, dict):
                raise ValueError('Expected a JSON object')
            cmd = msg.get('cmd')
            LOG.debug('IPC command: %s', cmd)
            with self._commands:
                if cmd == 'toggle_listening':
                    self.controller.set_listening(not self.controller.is_listening)
                elif cmd == 'set_listening':
                    if type(msg.get('value')) is not bool:
                        raise ValueError('set_listening requires a boolean value')
                    self.controller.set_listening(msg['value'])
                elif cmd == 'reload_config':
                    self.controller.reload_config()
                    setup_logging(self.controller.config)
                    self.broadcast({'type': 'config_reloaded'})
                elif cmd == 'set_log_level':
                    level = msg.get('value')
                    if level not in ('DEBUG', 'INFO'):
                        raise ValueError('Log level must be DEBUG or INFO')
                    self.controller.config.data['logging']['level'] = level
                    self.controller.config.save()
                    setup_logging(self.controller.config)
                    LOG.info('Log level changed to %s', level)
                    self.broadcast({'type': 'config_reloaded'})
                elif cmd == 'cancel':
                    self.controller.cancel()
                    self.broadcast({'type': 'cancelled'})
                elif cmd == 'get_state':
                    self.broadcast(self._state_msg())
                elif cmd == 'clear_error':
                    self.last_error = ''
                    self.broadcast(self._state_msg())
                elif cmd == 'quit':
                    os.kill(os.getpid(), signal.SIGTERM)
                else:
                    raise ValueError(f'Unknown command: {cmd}')
        except (ValueError, UnicodeError) as exc:
            LOG.warning('Invalid IPC command: %s', exc)
            self.broadcast({'type': 'error', 'message': str(exc)})
        except Exception as exc:
            LOG.exception('IPC command failed')
            self.broadcast({'type': 'error', 'message': str(exc)})


class STTDaemon:
    def __init__(self):
        self.config = Config()
        setup_logging(self.config)
        self._stop_event = threading.Event()
        self.history = HistoryManager()
        self.controller = SpeechToTextController(config=self.config, on_state=self._on_state,
            on_transcription=self._on_transcription, on_error=self._on_error, on_level=self._on_level)
        self.ipc = IPCServer(SOCKET_PATH, self.controller)

    def _on_state(self, *state):
        self.ipc.broadcast(self.ipc._state_msg())

    def _on_transcription(self, text, duration):
        try:
            self.history.add(text)
        except Exception:
            LOG.exception('Could not save transcription history')
        self.ipc.broadcast({'type': 'transcription', 'text': text, 'duration': duration})

    def _on_level(self, level, db):
        self.ipc.broadcast({'type': 'audio_level', 'level': level, 'db': db, 'capture_ready': True})

    def _on_error(self, message):
        self.ipc.broadcast({'type': 'error', 'message': message})

    def run(self):
        signal.signal(signal.SIGTERM, self._handle_signal)
        signal.signal(signal.SIGINT, self._handle_signal)
        self.ipc.start()
        try:
            if not self.controller.start():
                raise RuntimeError('Controller failed to start')
            LOG.info('Daemon started: model=%s beam=%s preprocess=%s session=%s',
                self.config.get('transcription', 'model'), self.config.get('transcription', 'beam_size'),
                self.config.get('audio', 'preprocess'), os.environ.get('XDG_SESSION_TYPE', 'unknown'))
            self._stop_event.wait()
        finally:
            LOG.info('Daemon shutting down')
            self.controller.stop()
            self.ipc.stop()

    def _handle_signal(self, sig, frame):
        self._stop_event.set()


def main():
    STTDaemon().run()


if __name__ == '__main__':
    main()
