#!/usr/bin/env python3
"""Native desktop connected to the Rust service, using temporary history and Xvfb."""
import argparse
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time
from PIL import ImageGrab


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', default='target/release')
    args = parser.parse_args()
    binaries = Path(args.directory).resolve()
    output = Path('artifacts/native').resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='voice-service-ui-') as directory:
        root = Path(directory)
        config = root/'.config/speech-to-text/config.yaml'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'output': {'method': 'none'}, 'notifications': {'enabled': False, 'audio_feedback': False}}))
        data = root/'.local/share/speech-to-text'
        data.mkdir(parents=True)
        with sqlite3.connect(data/'history.db') as db:
            db.execute('CREATE TABLE history (id INTEGER PRIMARY KEY AUTOINCREMENT,timestamp TEXT,text TEXT)')
            db.execute('INSERT INTO history(timestamp,text) VALUES (?,?)', ('2026-01-01', 'Synthetic Rust service transcript. Салом!'))
        processes = []
        with open(root/'display', 'w+') as display_file, open(output/'service-ui.log', 'w') as log:
            try:
                display = subprocess.Popen(['Xvfb', '-displayfd', str(display_file.fileno()), '-screen', '0', '1400x1000x24', '-nolisten', 'tcp'], pass_fds=(display_file.fileno(),), stdout=log, stderr=log)
                processes.append(display)
                for _ in range(100):
                    display_file.seek(0)
                    number = display_file.read().strip()
                    if number:
                        break
                    time.sleep(.05)
                else:
                    raise RuntimeError('Xvfb did not start')
                env = {**os.environ, 'HOME': str(root), 'STT_SOCKET_PATH': str(root/'run/daemon.sock'), 'DISPLAY': ':'+number, 'LIBGL_ALWAYS_SOFTWARE': '1'}
                env.pop('WAYLAND_DISPLAY', None)
                daemon = subprocess.Popen([str(binaries/'speech-service'), '--probe'], env=env, stdout=log, stderr=log)
                processes.append(daemon)
                for _ in range(100):
                    if Path(env['STT_SOCKET_PATH']).exists():
                        break
                    time.sleep(.05)
                app = subprocess.Popen([str(binaries/'voice-dictation')], env=env, stdout=log, stderr=log)
                processes.append(app)
                def x(*args):
                    return subprocess.check_output(['xdotool', *map(str, args)], env=env, stderr=subprocess.DEVNULL, timeout=5).decode().strip()
                for _ in range(100):
                    try:
                        window = x('search', '--onlyvisible', '--name', '^Voice Dictation$').splitlines()[-1]
                        break
                    except subprocess.CalledProcessError:
                        time.sleep(.05)
                else:
                    raise RuntimeError('Native window did not appear')
                x('windowmove', window, 0, 0)
                x('windowfocus', window)
                # Copy through the actual UI, after the adapter has loaded temporary history.
                for _ in range(30):
                    x('mousemove', '--window', window, 965, 403, 'click', 1)
                    time.sleep(.1)
                    copied = subprocess.run(['xclip', '-selection', 'clipboard', '-o'], env=env, capture_output=True, timeout=3).stdout.decode()
                    if copied == 'Synthetic Rust service transcript. Салом!':
                        break
                else:
                    raise AssertionError(f'Rust adapter did not populate the native transcript: {copied!r}')
                x('mousemove', '--window', window, 297, 329, 'click', 1)
                time.sleep(.3)
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(3)
                    client.connect(env['STT_SOCKET_PATH'])
                    state = json.loads(client.makefile('rb').readline())
                assert state['listening'] is False, state
                ImageGrab.grab(bbox=(0, 0, 1060, 860), xdisplay=env['DISPLAY']).save(output/'14-rust-service-connected.png')
                x('windowclose', window)
                app.wait(timeout=5)
                assert daemon.poll() is None, 'Closing the UI stopped the speech service'
                app = subprocess.Popen([str(binaries/'voice-dictation')], env=env, stdout=log, stderr=log)
                processes.append(app)
                time.sleep(1)
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(3)
                    client.connect(env['STT_SOCKET_PATH'])
                    client.sendall(b'{"cmd":"quit"}\n')
                assert daemon.wait(timeout=5) == 0
                assert app.wait(timeout=5) == 0
                time.sleep(.5)
                assert not Path(env['STT_SOCKET_PATH']).exists(), 'Desktop restarted a deliberately stopped service'
                print('Rust adapter, native transcript, Unicode clipboard, pause, independent lifetime and app quit passed')
            finally:
                for process in reversed(processes):
                    if process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait()


if __name__ == '__main__':
    main()
