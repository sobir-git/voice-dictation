"""Read-only runtime report. Never includes dictation text or audio."""
import importlib.metadata
import json
import os
import shutil
import socket
import subprocess
from pathlib import Path

from speech_to_text.core.app_logging import log_path
from speech_to_text.core.runtime import SOCKET_PATH


def command(args):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=3)
        return result.stdout.strip() if result.returncode == 0 else result.stderr.strip()
    except (OSError, subprocess.TimeoutExpired) as exc:
        return str(exc)


def microphones():
    try:
        sources = json.loads(command(['pactl', '-f', 'json', 'list', 'sources']))
        return [{'name': s['name'], 'description': s.get('description', s['name']),
                 'muted': s.get('mute', False)} for s in sources if not s['name'].endswith('.monitor')]
    except (ValueError, KeyError, TypeError):
        return []


def report(config):
    state = {'error': 'Daemon is not reachable'}
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(1)
            sock.connect(SOCKET_PATH)
            with sock.makefile('rb') as stream:
                state = json.loads(stream.readline(65536))
    except (OSError, ValueError):
        pass
    versions = {}
    for package in ('faster-whisper', 'ctranslate2', 'evdev', 'PyYAML'):
        try:
            versions[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            versions[package] = 'missing'
    return {'session': os.environ.get('XDG_SESSION_TYPE', 'unknown'), 'socket': SOCKET_PATH,
            'daemon': state, 'audio': config.data['audio'], 'transcription': config.data['transcription'],
            'default_microphone': command(['pactl', 'get-default-source']), 'microphones': microphones(),
            'tools': {name: shutil.which(name) for name in ('arecord', 'ffmpeg', 'xdotool', 'ydotool', 'wtype', 'dotool')},
            'versions': versions, 'logs': [str(log_path(config)), str(log_path(config, 'tray'))]}


def recent_logs(config, lines=120):
    result = []
    for role in ('daemon', 'tray'):
        path = log_path(config, role)
        if path.exists():
            # Read only the tail of large files.
            with path.open('rb') as f:
                f.seek(max(0, path.stat().st_size - 64000))
                tail = f.read().decode(errors='replace').splitlines()[-lines:]
            result.append(str(path) + '\n' + '\n'.join(tail))
    return '\n\n'.join(result)
