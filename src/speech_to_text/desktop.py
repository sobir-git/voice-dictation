"""Headless JSON adapter for the native desktop client. No toolkit dependency."""
import glob
import json
import selectors
import sys
import threading
import time
from copy import deepcopy
from queue import Queue, Full

from speech_to_text.core.config import Config
from speech_to_text.core.diagnostics import microphones, report, recent_logs
from speech_to_text.core.history import HistoryManager
from speech_to_text.core.ipc_client import DaemonClient
from speech_to_text.core.microphone_test import MicrophoneTest
from speech_to_text.core.runtime import SOCKET_PATH, ensure_daemon


class Desktop:
    def __init__(self, emit):
        self.emit = emit
        self.config = Config()
        self.history = HistoryManager()
        self.state = {}
        self.client = DaemonClient(SOCKET_PATH, self.event, reconnect=ensure_daemon)
        self.test = None
        self.stop = threading.Event()
        self.work = Queue(maxsize=16)
        self.saved_before = None
        self.saving = False
        self.latest_loaded = False

    def event(self, message):
        kind = message.get('type')
        if kind == 'state':
            self.state = message
        elif kind == 'config_reloaded':
            self.saving = False
            self.saved_before = None
            self.submit({'cmd': 'get_config'})
        elif kind == 'transcription':
            self.submit({'cmd': 'history'})
        elif kind == 'connected':
            self.submit({'cmd': 'get_config'})
        elif kind in ('error', 'disconnected') and self.saving:
            self.rollback()
        self.emit(message)

    def rollback(self):
        previous, self.saved_before = self.saved_before, None
        self.saving = False
        if previous is not None:
            self.config.data = previous
            try:
                self.config.save()
            except Exception as exc:
                self.emit({'type': 'error', 'message': f'Could not restore settings: {exc}'})

    def submit(self, message):
        try:
            self.work.put_nowait(message)
        except Full:
            self.emit({'type': 'error', 'message': 'Too many pending requests. Try again shortly.'})

    def command(self, message):
        cmd = message.get('cmd')
        if cmd in ('get_state', 'set_listening', 'toggle_listening', 'cancel', 'clear_error', 'set_log_level'):
            if not self.client.send(message):
                raise RuntimeError('Speech service is disconnected. Reconnecting…')
        elif cmd == 'get_config':
            if not self.saving:
                self.config.load()
                self.emit({'type': 'config', 'config': self.config.data, 'microphones': microphones()})
                if not self.latest_loaded:
                    self.latest_loaded = True
                    latest = self.history.get_recent(1)
                    if latest:
                        self.emit({'type': 'transcription', 'text': latest[0]['text']})
        elif cmd == 'history':
            query = str(message.get('search', ''))[:4096]
            self.emit({'type': 'history', 'search': query, 'items': self.history.get_recent(100, query)})
        elif cmd == 'diagnostics':
            self.emit({'type': 'diagnostics', 'text': json.dumps(report(self.config), indent=2, ensure_ascii=False),
                       'logs': recent_logs(self.config)})
        elif cmd == 'save_config':
            if self.saving or self.state.get('recording') or self.state.get('processing') or self.state.get('pending'):
                raise RuntimeError('Wait for dictation or the current settings change to finish.')
            candidate = deepcopy(self.config.data)
            changes = message.get('config')
            if not isinstance(changes, dict):
                raise ValueError('Expected settings sections')
            self.config._deep_update(candidate, changes)
            Config.validate(candidate)
            self.saved_before = deepcopy(self.config.data)
            self.config.data = candidate
            try:
                self.config.save()
                self.saving = True
                if not self.client.send({'cmd': 'reload_config'}):
                    raise RuntimeError('Speech service is disconnected; settings were not applied.')
            except Exception:
                self.rollback()
                raise
        elif cmd == 'test_microphone':
            if self.state.get('recording') or self.state.get('processing'):
                raise RuntimeError('Finish dictation before testing the microphone.')
            if self.test:
                self.test.stop()
                self.test = None
                self.emit({'type': 'microphone_test', 'level': 0, 'message': 'Microphone test stopped.', 'done': True})
                return
            node = str(message.get('node', self.config.get('audio', 'pipewire_node')))
            def update(level, text, done):
                self.emit({'type': 'microphone_test', 'level': level, 'message': text, 'done': done})
                if done:
                    self.test = None
            self.test = MicrophoneTest('pipewire' if node else 'default', node, update)
            self.test.start()
        elif cmd == 'capture_hotkey':
            self.capture_hotkey()
        else:
            raise ValueError(f'Unknown desktop command: {cmd}')

    def capture_hotkey(self):
        from evdev import InputDevice, ecodes
        if self.state.get('recording') or self.state.get('processing'):
            raise RuntimeError('Finish dictation before changing the hotkey.')
        listening = self.state.get('listening', True)
        if not self.client.send({'cmd': 'set_listening', 'value': False}):
            raise RuntimeError('Connect to the speech service first.')
        devices = []
        try:
            with selectors.DefaultSelector() as selector:
                for path in sorted(glob.glob('/dev/input/event*')):
                    try:
                        device = InputDevice(path)
                        if ecodes.EV_KEY in device.capabilities():
                            devices.append(device)
                            selector.register(device, selectors.EVENT_READ)
                        else:
                            device.close()
                    except OSError:
                        pass
                if not devices:
                    raise RuntimeError('No readable keyboard. Check membership in the input group.')
                self.emit({'type': 'hotkey_waiting'})
                deadline = time.monotonic() + 10
                while not self.stop.is_set() and time.monotonic() < deadline:
                    for key, _ in selector.select(.1):
                        try:
                            events = key.fileobj.read()
                            for event in events:
                                if event.type == ecodes.EV_KEY and event.value == 1:
                                    name = ecodes.KEY.get(event.code)
                                    if isinstance(name, list):
                                        name = name[0]
                                    if name:
                                        self.emit({'type': 'hotkey', 'key': name})
                                        return
                        except OSError:
                            pass
                self.emit({'type': 'hotkey_timeout'})
        finally:
            for device in devices:
                device.close()
            self.client.send({'cmd': 'set_listening', 'value': listening})

    def worker(self):
        while not self.stop.is_set():
            message = self.work.get()
            if message is None:
                break
            try:
                self.command(message)
            except Exception as exc:
                self.emit({'type': 'error', 'message': str(exc)})

    def run(self):
        self.client.start()
        worker = threading.Thread(target=self.worker, name='desktop-work', daemon=True)
        worker.start()
        try:
            for line in sys.stdin:
                if len(line) > 65536:
                    self.emit({'type': 'error', 'message': 'Desktop request exceeds 64 KiB'})
                    continue
                try:
                    msg = json.loads(line)
                    if not isinstance(msg, dict):
                        raise ValueError('Expected a JSON object')
                    self.submit(msg)
                except ValueError as exc:
                    self.emit({'type': 'error', 'message': str(exc)})
        finally:
            self.stop.set()
            if self.test:
                self.test.stop()
            try:
                self.work.put_nowait(None)
            except Full:
                pass
            # Key capture restores the previous listening state through this client.
            worker.join(timeout=12)
            self.client.stop()


def main():
    lock = threading.Lock()
    def emit(message):
        with lock:
            try:
                print(json.dumps(message, ensure_ascii=False), flush=True)
            except BrokenPipeError:
                pass
    Desktop(emit).run()


if __name__ == '__main__':
    main()
