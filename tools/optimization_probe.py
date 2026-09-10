#!/usr/bin/env python3
"""Exercise the terminal benchmark through an isolated daemon and synthetic audio.
Requires the already-cached Whisper base.en model. Never reads user recordings.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--gpu', action='store_true')
    args = parser.parse_args()
    profile = 'vulkan' if args.gpu else 'fast'
    model = 'parakeet-unified-en-0.6b' if args.gpu else 'base.en'
    binary = Path('target/release/speech-service').resolve()
    cache = Path.home()/'.cache/huggingface'
    cached = list((cache/'hub/models--handy-computer--parakeet-unified-en-0.6b-gguf/snapshots').glob('*/*.gguf')) if args.gpu else list((cache/'hub/models--Systran--faster-whisper-base.en/snapshots').glob('*/model.bin'))
    assert cached, 'Cache the selected model before running this probe'
    with tempfile.TemporaryDirectory(prefix='optimization-probe-') as directory:
        root = Path(directory)
        path = root/'.config/speech-to-text/config.yaml'
        path.parent.mkdir(parents=True)
        config = {'transcription': {'model': model, 'compute_type': 'int8', 'language': 'en', 'beam_size': 5, 'vad_filter': True}, 'performance': {'profile': 'standard', 'threads': 0}, 'notifications': {'enabled': False, 'audio_feedback': False}, 'output': {'method': 'none'}, 'input': {'trigger_key': 'KEY_F15'}, 'logging': {'file': str(root/'service.log')}}
        path.write_text(json.dumps(config))
        original = path.read_bytes()
        env = {**os.environ, 'HOME': str(root), 'HF_HOME': str(cache), 'STT_SOCKET_PATH': str(root/'run/daemon.sock'), 'DBUS_SESSION_BUS_ADDRESS': 'unix:path='+str(root/'no-dbus')}
        (root/'.cache').mkdir(); (root/'.cache/huggingface').symlink_to(cache, target_is_directory=True)
        def command(*args, timeout=60):
            p = subprocess.run([str(binary), *args], env=env, capture_output=True, text=True, timeout=timeout)
            assert p.returncode == 0, (args, p.stderr)
            return json.loads(p.stdout)
        def ready():
            for _ in range(1200):
                if daemon.poll() is not None:
                    raise AssertionError((root/'daemon.log').read_text()[-3000:])
                if Path(env['STT_SOCKET_PATH']).exists():
                    with socket.socket(socket.AF_UNIX) as sock:
                        sock.settimeout(2); sock.connect(env['STT_SOCKET_PATH'])
                        state = json.loads(sock.makefile().readline())
                        if state.get('model_ready') and not state.get('benchmarking'):
                            return state
                time.sleep(.1)
            raise AssertionError('Isolated daemon did not become ready')
        with (root/'daemon.log').open('w') as log:
            daemon = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                ready()
                capabilities = command('--list-optimizations')
                assert capabilities['schema_version'] == 1
                report = command('--benchmark', '--profile', profile, '--threads', '2', '--seconds', '2')
                assert report['performance']['profile'] == profile
                assert report['performance']['threads'] == 2
                row = report['rows'][0]
                assert row['reference']['warm_seconds'] > 0 and row['candidate']['process_peak_mib'] > 0
                assert path.read_bytes() == original, 'Benchmark changed user configuration'
                ready()
                saved = command('--apply-optimization', profile, '--threads', '2')
                assert saved['saved']
                state = ready()
                assert state['performance']['profile'] == profile, state
                assert state['performance']['threads'] == 2, state
                assert state['performance']['by_model'][model] == {'profile': profile, 'threads': 2}, state
                assert state['runtime_profile'] == profile, state
                assert len(command('--benchmark-results')) == 1
                # Cancel a long suite without waiting for long inference.
                with (root/'cancel.out').open('w') as out, (root/'cancel.err').open('w') as err:
                    run = subprocess.Popen([str(binary), '--benchmark', '--profile', profile, '--seconds', '60'], env=env, stdout=out, stderr=err)
                    try:
                        for _ in range(100):
                            if 'reference' in (root/'cancel.err').read_text(): break
                            assert run.poll() is None
                            time.sleep(.05)
                        else: raise AssertionError('Benchmark did not start')
                        run.send_signal(signal.SIGINT)
                        assert run.wait(timeout=10) != 0
                    finally:
                        if run.poll() is None: run.kill(); run.wait()
                state = ready()
                assert state['runtime_profile'] == profile
                assert len(command('--benchmark-results')) == 1, 'Cancelled benchmark saved a completed report'
                databases = list((root/'.local/share/speech-to-text').glob('*.db'))
                assert databases
                with sqlite3.connect(databases[0]) as db:
                    assert db.execute('select count(*) from history').fetchone()[0] == 0
                assert not list((root/'.local/share/speech-to-text').rglob('*.wav'))
                print(json.dumps({'passed': True, 'profile': profile, 'same_words': row['same_words'], 'speedup': row['speedup'], 'candidate_process_mib': row['candidate']['process_peak_mib'], 'cancel_restored_engine': True, 'history_unchanged': True}))
            finally:
                daemon.terminate()
                try: daemon.wait(timeout=10)
                except subprocess.TimeoutExpired: daemon.kill(); daemon.wait()


if __name__ == '__main__':
    main()
