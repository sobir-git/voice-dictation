"""Fault tests with generated audio, an isolated daemon and no speech model."""
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import unittest
import wave

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('STT_TEST_BINARY', ROOT/'target/debug/speech-service'))


class RecordingPreservationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='dictation-preservation-')
        self.root = Path(self.temp.name)
        config = self.root/'.config/speech-to-text/config.yaml'
        config.parent.mkdir(parents=True)
        model = self.root/'empty-model'
        model.mkdir()  # Deterministic model-load failure, without downloads.
        config.write_text(json.dumps({
            'transcription': {'model': str(model)}, 'output': {'method': 'none'},
            'notifications': {'enabled': False, 'audio_feedback': False}}))
        self.data = self.root/'.local/share/speech-to-text'
        binaries = self.root/'bin'
        binaries.mkdir()
        recorder = binaries/'arecord'
        recorder.write_text('''#!/usr/bin/python3
import os, signal, sys, time, wave
running = True
def stop(*_):
    global running
    running = False
signal.signal(signal.SIGINT, stop)
with open(os.environ['RECORDER_PID'], 'w') as pid:
    pid.write(str(os.getpid()))
with open(sys.argv[-1], 'wb') as file:
    with wave.open(file, 'wb') as wav:
        wav.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
        while running:
            wav.writeframesraw(b'\\x64\\x00' * 16000)
            file.flush()
            time.sleep(.01)
''')
        recorder.chmod(0o700)
        self.env = {**os.environ, 'HOME': str(self.root),
                    'STT_SOCKET_PATH': str(self.root/'daemon.sock'),
                    'RECORDER_PID': str(self.root/'recorder.pid'),
                    'PATH': str(binaries)+os.pathsep+os.environ['PATH']}
        self.log = (self.root/'daemon.log').open('w')
        self.sockets = []
        self.daemon = None
        self.launch()

    def wait(self, predicate, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(.01)
        self.fail('Timed out waiting for recording state')

    def launch(self):
        self.daemon = subprocess.Popen([str(BINARY), '--probe'], env=self.env,
                                       stdout=self.log, stderr=self.log)
        def connect():
            client = socket.socket(socket.AF_UNIX)
            try:
                client.connect(self.env['STT_SOCKET_PATH'])
                client.settimeout(5)
                self.sockets.append(client)
                self.client = client
                return True
            except OSError:
                client.close()
                return False
        self.wait(connect)
        self.receive('state')

    def receive(self, kind):
        # Read exactly one line so subsequent calls never lose buffered messages.
        while True:
            line = bytearray()
            while not line.endswith(b'\n'):
                chunk = self.client.recv(1)
                self.assertTrue(chunk, 'Daemon disconnected')
                line.extend(chunk)
            message = json.loads(line)
            if message['type'] == 'error' and kind == 'recording_started':
                self.fail(message.get('message', 'Daemon command failed'))
            if message['type'] == kind:
                return message

    def command(self, cmd, response):
        message = {'cmd': cmd}
        if cmd == 'start_recording':
            message['pipewire_node'] = 'synthetic'
        self.client.sendall((json.dumps(message)+'\n').encode())
        return self.receive(response)

    def start_capture(self):
        self.command('start_recording', 'recording_started')
        def captured():
            paths = list((self.data/'recordings').glob('dictation-*.wav'))
            return paths[0] if paths and paths[0].stat().st_size >= 32044 else None
        return self.wait(captured)

    def rows(self):
        with sqlite3.connect(f'file:{self.data}/history.db?mode=ro', uri=True) as db:
            return db.execute('SELECT audio_path,duration,failed FROM history').fetchall()

    def assert_retained(self, path, minimum):
        rows = self.wait(self.rows)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][0], str(path))
        self.assertTrue(path.exists())
        with wave.open(str(path)) as wav:
            self.assertGreaterEqual(wav.getnframes(), minimum)
        return rows

    def test_disconnect_saves_audio_captured_so_far(self):
        path = self.start_capture()
        minimum = (path.stat().st_size - 44)//2
        self.client.close()
        self.assert_retained(path, minimum)

    def test_abort_retains_audio_for_retry(self):
        path = self.start_capture()
        minimum = (path.stat().st_size - 44)//2
        self.command('abort_recording', 'recording_stopped')
        rows = self.assert_retained(path, minimum)
        self.assertEqual(rows[0][2], 1)

    def test_three_minutes_survive_transcription_failure(self):
        path = self.start_capture()
        def recorded_three_minutes():
            self.command('get_state', 'state')  # Drain telemetry as the phone bridge does.
            return path.stat().st_size >= 44 + 180 * 16000 * 2
        self.wait(recorded_three_minutes)
        minimum = (path.stat().st_size - 44)//2
        self.command('stop_recording', 'recording_stopped')
        self.assert_retained(path, minimum)
        self.wait(lambda: not self.command('get_state', 'state').get('jobs'))
        rows = self.rows()
        self.assertEqual(rows[0][2], 1)  # Missing model failed, audio remains retryable.
        self.assertGreaterEqual(rows[0][1], 180)
        self.assertTrue(path.exists())

    def test_sigkill_recovers_unfinished_wav_once_on_restart(self):
        path = self.start_capture()
        minimum = (path.stat().st_size - 44)//2
        self.daemon.kill()
        self.daemon.wait(timeout=5)
        self.client.close()
        self.assertTrue(path.exists())
        self.launch()
        self.assert_retained(path, minimum)
        self.daemon.terminate()
        self.daemon.wait(timeout=12)
        self.client.close()
        self.launch()
        self.assertEqual(len(self.rows()), 1)

    def test_recorder_crash_repairs_header_and_retains_audio(self):
        path = self.start_capture()
        minimum = (path.stat().st_size - 44)//2
        os.kill(int((self.root/'recorder.pid').read_text()), signal.SIGKILL)
        self.assert_retained(path, minimum)

    def tearDown(self):
        for client in self.sockets:
            client.close()
        if self.daemon and self.daemon.poll() is None:
            self.daemon.terminate()
            try:
                self.daemon.wait(timeout=12)
            except subprocess.TimeoutExpired:
                self.daemon.kill()
                self.daemon.wait(timeout=5)
        self.log.close()
        self.temp.cleanup()
