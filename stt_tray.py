#!/usr/bin/env python3

import os
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / 'src'))

from speech_to_text.gui.ipc_client import SOCKET_PATH as _DEFAULT_SOCKET_PATH
from speech_to_text.tray import main


def get_socket_path() -> str:
    return os.environ.get('STT_SOCKET_PATH', _DEFAULT_SOCKET_PATH)


def _socket_ready(socket_path: str) -> bool:
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(0.5)
        sock.connect(socket_path)
        sock.close()
        return True
    except Exception:
        return False


def _ensure_daemon() -> None:
    socket_path = get_socket_path()
    if _socket_ready(socket_path):
        return

    script_dir = Path(__file__).resolve().parent
    daemon_cmd = [sys.executable, str(script_dir / 'stt_daemon.py')]
    subprocess.Popen(daemon_cmd)

    deadline = time.time() + 20
    while time.time() < deadline:
        if _socket_ready(socket_path):
            return
        time.sleep(0.2)

    raise RuntimeError('Speech-to-text daemon did not start')


if __name__ == '__main__':
    _ensure_daemon()
    raise SystemExit(main())
