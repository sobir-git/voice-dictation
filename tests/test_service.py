"""Rust service integration tests using temporary configuration and synthetic history."""
import json
import os
from pathlib import Path
import socket
import select
import sqlite3
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('STT_TEST_BINARY', ROOT/'target/debug/speech-service'))


class ServiceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='voice-rust-test-')
        self.root = Path(self.temp.name)
        self.config = self.root/'.config/speech-to-text/config.yaml'
        self.config.parent.mkdir(parents=True)
        self.config.write_text(json.dumps({'output': {'method': 'none'}, 'custom': {'preserve': 17}, 'notifications': {'enabled': False, 'audio_feedback': False}}))
        self.env = {**os.environ, 'HOME': str(self.root), 'STT_SOCKET_PATH': str(self.root/'run/daemon.sock')}
        fake_bin = self.root/'bin'
        fake_bin.mkdir()
        recorder = fake_bin/'arecord'
        recorder.write_text('#!/bin/sh\nif [ "$PIPEWIRE_NODE" = slow ]; then exec sleep 30; fi\nhead -c 6400 /dev/zero\n')
        recorder.chmod(0o700)
        self.env['PATH'] = str(fake_bin) + os.pathsep + os.environ['PATH']
        self.log = open(self.root/'service.log', 'w+')
        self.daemon = subprocess.Popen([str(BINARY), '--probe'], env=self.env, stdout=self.log, stderr=self.log)
        self.clients = []
        for _ in range(100):
            try:
                self.client = self.connect()
                break
            except (OSError, ValueError):
                if self.daemon.poll() is not None:
                    self.log.seek(0)
                    self.fail(self.log.read())
                time.sleep(.05)
        else:
            self.fail('Rust service did not start')

    def connect(self):
        client = socket.socket(socket.AF_UNIX)
        client.settimeout(3)
        try:
            client.connect(self.env['STT_SOCKET_PATH'])
        except OSError:
            client.close()
            raise
        stream = client.makefile('rwb', buffering=0)
        state = json.loads(stream.readline())
        self.assertEqual(state['protocol'], 2)
        self.clients.append((client, stream))
        return stream

    def request(self, message, kind, client=None):
        stream = client or self.client
        stream.write((json.dumps(message)+'\n').encode())
        for _ in range(100):
            result = json.loads(stream.readline())
            if result['type'] == kind:
                return result
        self.fail('No matching reply')

    def tearDown(self):
        for client, stream in self.clients:
            stream.close()
            client.close()
        self.daemon.terminate()
        try:
            self.daemon.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.daemon.kill()
            self.daemon.wait()
        self.log.close()
        self.temp.cleanup()

    def test_remote_recording_ownership_and_temporary_microphone(self):
        # Keep a synthetic WAV growing, just like the recorder, without opening a mic.
        recorder = self.root/'bin/arecord'
        recorder.write_text("#!/usr/bin/python3\nimport os,sys,time,wave\n"
                            "open(os.environ['STT_SOCKET_PATH']+'.node','w').write(os.environ.get('PIPEWIRE_NODE',''))\n"
                            "with wave.open(sys.argv[-1], 'wb') as wav:\n"
                            " wav.setparams((1,2,16000,0,'NONE','not compressed'))\n"
                            " while True:\n  wav.writeframes(b'\\0'*3200); time.sleep(.05)\n")
        original = self.request({'cmd': 'get_config'}, 'config')['config']
        self.request({'cmd': 'start_recording', 'pipewire_node': 'phonemic2_src'}, 'recording_started')
        deadline = time.monotonic() + 2
        node_file = Path(self.env['STT_SOCKET_PATH']+'.node')
        while not node_file.exists() and time.monotonic() < deadline:
            time.sleep(.02)
        self.assertEqual(node_file.read_text(), 'phonemic2_src')
        other = self.connect()
        error = self.request({'cmd': 'stop_recording'}, 'error', other)
        self.assertIn('own', error['message'])
        self.request({'cmd': 'abort_recording'}, 'recording_stopped')
        state = self.request({'cmd': 'get_state'}, 'state')
        self.assertFalse(state['recording'])
        self.assertFalse(state['processing'])
        current = self.request({'cmd': 'get_config'}, 'config')['config']
        self.assertEqual(current['audio'], original['audio'])
        self.assertEqual(json.loads(self.config.read_text())['custom']['preserve'], 17)

    def test_rust_adapter_connects_and_exits_without_stopping_service(self):
        adapter = subprocess.Popen([str(BINARY), '--adapter'], env=self.env,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=self.log, bufsize=0)
        try:
            messages = []
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if select.select([adapter.stdout], [], [], .2)[0]:
                    messages.append(json.loads(adapter.stdout.readline()))
                if any(m['type'] == 'config' for m in messages):
                    break
            self.assertTrue(any(m['type'] == 'connected' for m in messages))
            self.assertTrue(any(m['type'] == 'config' for m in messages))
            adapter.stdin.write(b'{"cmd":"set_listening","value":false}\n')
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if select.select([adapter.stdout], [], [], .2)[0]:
                    message = json.loads(adapter.stdout.readline())
                    if message['type'] == 'state' and not message['listening']:
                        break
            else:
                self.fail('Adapter did not deliver pause command')
            adapter.stdin.close()
            self.assertEqual(adapter.wait(timeout=3), 0)
            self.assertIsNone(self.daemon.poll())
            self.assertFalse(self.request({'cmd': 'get_state'}, 'state')['listening'])
        finally:
            if adapter.poll() is None:
                adapter.kill()
                adapter.wait()
            adapter.stdin.close()
            adapter.stdout.close()

    def test_microphone_probe_uses_memory_and_cancels_stalled_recorder(self):
        message = self.request({'cmd': 'test_microphone'}, 'microphone_test')
        while not message['done']:
            message = json.loads(self.client.readline())
        self.assertIn('No signal', message['message'])
        history = self.request({'cmd': 'history'}, 'history')
        self.assertEqual(history['items'], [])
        self.assertEqual(list(self.root.rglob('*.wav')), [])
        self.client.write(b'{"cmd":"test_microphone","node":"slow"}\n')
        time.sleep(.2)
        started = time.monotonic()
        message = self.request({'cmd': 'test_microphone'}, 'microphone_test')
        self.assertTrue(message['done'])
        self.assertLess(time.monotonic() - started, 2)
        self.assertEqual(self.request({'cmd': 'get_state'}, 'state')['pending'], 0)

    def test_atomic_settings_and_unknown_values(self):
        before = self.config.read_bytes()
        error = self.request({'cmd': 'save_config', 'config': {'audio': {'channels': True}}}, 'error')
        self.assertIn('integer', error['message'])
        self.assertEqual(self.config.read_bytes(), before)
        self.request({'cmd': 'save_config', 'config': {'ui': {'cursor_indicator': True}}}, 'config_reloaded')
        result = self.request({'cmd': 'get_config'}, 'config')
        self.assertTrue(result['config']['ui']['cursor_indicator'])
        self.assertEqual(result['config']['custom']['preserve'], 17)

    def test_history_retains_python_schema_and_search(self):
        db = sqlite3.connect(self.root/'.local/share/speech-to-text/history.db')
        db.executemany('INSERT INTO history(timestamp,text) VALUES (?,?)', [('2026-01-01', 'Synthetic first note'), ('2026-01-02', 'Салом Rust')])
        db.commit()
        db.close()
        result = self.request({'cmd': 'history', 'search': 'RUST'}, 'history')
        self.assertEqual([r['text'] for r in result['items']], ['Салом Rust'])

    def test_pause_cancel_and_bad_commands_keep_service_available(self):
        state = self.request({'cmd': 'set_listening', 'value': False}, 'state')
        self.assertFalse(state['listening'])
        self.request({'cmd': 'cancel'}, 'cancelled')
        self.request({'cmd': 'set_listening', 'value': 'yes'}, 'error')
        state = self.request({'cmd': 'get_state'}, 'state')
        self.assertFalse(state['listening'])
        self.assertFalse(state['recording'])

    def test_singleton_does_not_unlink_running_socket(self):
        duplicate = subprocess.run([str(BINARY), '--probe'], env=self.env, capture_output=True, timeout=5)
        self.assertNotEqual(duplicate.returncode, 0)
        self.assertIn('already running', duplicate.stderr.decode())
        self.assertEqual(self.request({'cmd': 'get_state'}, 'state')['protocol'], 2)
        self.connect()

    def test_partial_commands_survive_read_timeouts(self):
        self.client.write(b'{"cmd":"get_')
        time.sleep(.4)
        self.client.write(b'state"}\n')
        state = json.loads(self.client.readline())
        self.assertEqual(state['type'], 'state')

    def test_oversized_client_does_not_break_another_client(self):
        second = self.connect()
        try:
            second.write(b'x'*70000)
        except BrokenPipeError:
            pass
        self.assertEqual(self.request({'cmd': 'get_state'}, 'state')['protocol'], 2)


if __name__ == '__main__':
    unittest.main()
