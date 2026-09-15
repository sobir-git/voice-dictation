#!/usr/bin/env python3
"""Exercise real inference crash/recovery using a cached model and generated audio.
No microphone, network download, user configuration or user history is used.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--profile', default='standard', choices=['standard', 'vulkan'])
    parser.add_argument('--output', default='artifacts/lifecycle')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    model = Path(args.model).absolute()
    if not model.is_file():
        parser.error("The model must be an existing cached GGUF file")
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='voice-lifecycle-') as temporary:
        root = Path(temporary)
        config = root/'.config/speech-to-text/config.yaml'
        config.parent.mkdir(parents=True)
        configuration = {'transcription': {'model': str(model)},
                         'performance': {'profile': args.profile},
                         'output': {'method': 'none'},
                         'notifications': {'enabled': False, 'audio_feedback': False}}
        config.write_text(json.dumps(configuration))
        before = config.read_bytes()
        fake_bin = root/'bin'
        fake_bin.mkdir()
        recorder = fake_bin/'arecord'
        recorder.write_text('''#!/usr/bin/python3
import math, signal, struct, sys, time, wave
running = True
def stop(*_):
    global running
    running = False
signal.signal(signal.SIGINT, stop)
with wave.open(sys.argv[-1], 'wb') as wav:
    wav.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
    frame = b''.join(struct.pack('<h', int(1000*math.sin(2*math.pi*220*i/16000))) for i in range(1600))
    while running:
        wav.writeframes(frame)
        time.sleep(.1)
''')
        recorder.chmod(0o700)
        env = {**os.environ, 'HOME': str(root), 'STT_SOCKET_PATH': str(root/'daemon.sock'),
               'PATH': str(fake_bin)+os.pathsep+os.environ['PATH']}
        with (output/'daemon.log').open('w') as log:
            daemon = subprocess.Popen([str(binary), '--probe'], env=env, stdout=log, stderr=log)
            client = None
            try:
                for _ in range(100):
                    client = socket.socket(socket.AF_UNIX)
                    try:
                        client.connect(env['STT_SOCKET_PATH'])
                        break
                    except OSError:
                        client.close()
                        if daemon.poll() is not None:
                            raise AssertionError('Daemon exited during startup; see daemon.log')
                        time.sleep(.05)
                else:
                    raise AssertionError('Daemon socket did not appear')
                client.settimeout(15)
                stream = client.makefile('rwb', buffering=0)
                json.loads(stream.readline())
                serial = 0
                def request(command, kind):
                    nonlocal serial
                    serial += 1
                    command = {**command, 'request_id': serial}
                    stream.write((json.dumps(command)+'\n').encode())
                    while True:
                        raw = stream.readline()
                        assert raw, 'Daemon disconnected'
                        message = json.loads(raw)
                        if message.get('request_id') == serial and message['type'] == 'error':
                            raise AssertionError(message)
                        if message['type'] == kind and message.get('request_id') == serial:
                            return message
                def wait_state(predicate, seconds=30):
                    deadline = time.monotonic()+seconds
                    while time.monotonic() < deadline:
                        state = request({'cmd': 'get_state'}, 'state')
                        if predicate(state):
                            return state
                        time.sleep(.05)
                    raise AssertionError(f'State did not settle: {state}')
                def workers():
                    found = set()
                    for children in Path(f'/proc/{daemon.pid}/task').glob('*/children'):
                        for pid in children.read_text().split():
                            try:
                                cmdline = Path(f'/proc/{pid}/cmdline').read_bytes()
                                if b'--inference-worker' in cmdline:
                                    found.add(int(pid))
                            except FileNotFoundError:
                                pass
                    return found
                request({'cmd': 'start_recording', 'pipewire_node': 'synthetic'}, 'recording_started')
                first = wait_state(lambda s: s['model_ready'] and s['capture_ready'])
                time.sleep(.4)
                old_workers = workers()
                assert len(old_workers) == 1, old_workers
                os.kill(next(iter(old_workers)), signal.SIGKILL)
                failed = wait_state(lambda s: not s['recording'] and not s['processing'])
                assert failed['last_error'], failed
                assert daemon.poll() is None, 'Inference crash killed daemon'
                history = request({'cmd': 'history'}, 'history')['items']
                assert len(history) == 1 and history[0]['failed'], history
                assert Path(history[0]['audio_path']).is_file(), history
                started = time.monotonic()
                request({'cmd': 'get_state'}, 'state')
                latency = time.monotonic()-started
                assert latency < .5, latency
                request({'cmd': 'start_recording', 'pipewire_node': 'synthetic'}, 'recording_started')
                wait_state(lambda s: s['model_ready'] and s['capture_ready'])
                time.sleep(.4)
                new_workers = workers()
                assert len(new_workers) == 1 and new_workers.isdisjoint(old_workers), (old_workers, new_workers)
                request({'cmd': 'stop_recording'}, 'recording_stopped')
                recovered = wait_state(lambda s: not s['recording'] and not s['processing'])
                assert 'worker' not in recovered['last_error'].lower(), recovered
                history = request({'cmd': 'history'}, 'history')['items']
                assert len(history) == 2 and all(Path(row['audio_path']).is_file() for row in history), history
                assert before == config.read_bytes(), 'Probe modified configuration'
                report = {'profile': args.profile, 'runtime_profile': first['runtime_profile'],
                          'worker_restarted': True, 'recordings_preserved': len(history),
                          'state_latency_seconds': latency, 'pending_after_recovery': recovered['pending']}
                (output/'report.json').write_text(json.dumps(report, indent=2))
                print(json.dumps(report), flush=True)
                stream.close()
            finally:
                if client is not None:
                    client.close()
                daemon.terminate()
                try:
                    daemon.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait()


if __name__ == '__main__':
    main()
