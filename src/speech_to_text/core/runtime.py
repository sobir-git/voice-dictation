"""Shared IPC address, single-instance lock, and daemon launcher. No GTK imports."""
import fcntl
import json
import logging
import os
import socket
import subprocess
import sys
import time
from pathlib import Path


def socket_path():
    if os.environ.get('STT_SOCKET_PATH'):
        return os.environ['STT_SOCKET_PATH']
    runtime = Path(os.environ.get('XDG_RUNTIME_DIR', f'/tmp/speech-to-text-{os.getuid()}'))
    return str(runtime / 'speech-to-text' / 'daemon.sock')


SOCKET_PATH = socket_path()


def socket_ready(path=SOCKET_PATH):
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(0.5)
            sock.connect(path)
            with sock.makefile('rb') as stream:
                return json.loads(stream.readline(65536)).get('type') == 'state'
    except (OSError, ValueError, AttributeError):
        return False


class InstanceLock:
    def __init__(self, path):
        directory = Path(path).parent
        directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        self.file = open(path + '.lock', 'a')
        try:
            fcntl.flock(self.file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.file.close()
            raise RuntimeError('Speech-to-text daemon is already running')

    def close(self):
        self.file.close()


def ensure_daemon():
    if socket_ready():
        return
    project = Path(__file__).resolve().parents[3]
    service = Path.home() / '.config/systemd/user/speech-to-text-daemon.service'
    process = None
    if service.exists() and not os.environ.get('STT_SOCKET_PATH'):
        subprocess.run(['systemctl', '--user', 'start', '--no-block', 'speech-to-text-daemon.service'],
                       check=True, timeout=5, capture_output=True)
    else:
        process = subprocess.Popen([sys.executable, str(project / 'stt_daemon.py')], start_new_session=True)
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if socket_ready():
            return
        if process is not None and process.poll() is not None:
            # Another launcher may have won the singleton lock.
            time.sleep(0.2)
            if socket_ready():
                return
            raise RuntimeError(f'Daemon exited with code {process.returncode}; see app.log')
        time.sleep(0.2)
    raise RuntimeError('Daemon did not become ready within 20 seconds; see app.log')
