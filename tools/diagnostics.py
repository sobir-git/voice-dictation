#!/usr/bin/env python3
"""Collect a private, bounded support bundle, including when the daemon is down.
Never includes history databases, audio, model files, environment variables or core dumps.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import zipfile

FILE_LIMIT = 4 * 1024 * 1024
TOTAL_LIMIT = 48 * 1024 * 1024


def command(args):
    try:
        # A temporary file bounds memory even if a system command is very verbose.
        import tempfile
        with tempfile.TemporaryFile() as output:
            child = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=output,
                                     stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                import signal
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
                code = 'timeout'
            length = output.tell()
            output.seek(max(0, length-FILE_LIMIT))
            return {'command': args, 'exit_code': code, 'truncated': length > FILE_LIMIT,
                    'output': output.read(FILE_LIMIT).decode(errors='replace')}
    except OSError as error:
        return {'command': args, 'unavailable': str(error)}


def daemon_state():
    path = os.environ.get('STT_SOCKET_PATH') or str(Path(os.environ.get('XDG_RUNTIME_DIR', f'/tmp/speech-to-text-{os.getuid()}'))/'speech-to-text/daemon.sock')
    try:
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(path)
            with client.makefile('rb') as stream:
                state = json.loads(stream.readline(1_048_576))
                # Initial state contains no transcript or configuration payload.
                return state
    except (OSError, ValueError) as error:
        return {'unavailable': str(error), 'socket': path}


def log_files(path):
    files = [path] + [Path(str(path)+f'.{i}') for i in range(1, 6)]
    if path.parent.is_dir():
        for candidate in path.parent.iterdir():
            name = candidate.name.split('.log', 1)[0]
            role, _, pid = name.rpartition('-')
            if role in ('desktop', 'hud', 'adapter', 'cli') and pid.isdigit() and candidate.is_file() and not candidate.is_symlink():
                files.append(candidate)
    return sorted(set(files), key=lambda p: p.stat().st_mtime if p.exists() else 0, reverse=True)


def collect(output, log_path, binary_dir, system=True):
    output.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation, private permissions; never overwrite an existing export.
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    manifest = {'schema': 1, 'created_unix_ms': int(time.time()*1000), 'files': [],
                'limits': {'per_file_bytes': FILE_LIMIT, 'total_log_bytes': TOTAL_LIMIT},
                'excluded': ['history', 'audio', 'models', 'environment', 'core contents'],
                'privacy': 'Logs and system metadata can contain paths or native error text. Review before sharing.'}
    remaining = TOTAL_LIMIT
    with os.fdopen(fd, 'wb') as raw, zipfile.ZipFile(raw, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path in log_files(log_path):
            entry = {'source': str(path)}
            try:
                if path.is_symlink():
                    entry['unavailable'] = 'symlink skipped'
                else:
                    with path.open('rb') as stream:
                        size = os.fstat(stream.fileno()).st_size
                        length = min(size, FILE_LIMIT, remaining)
                        stream.seek(size-length)
                        data = stream.read(length)
                    entry.update({'bytes': len(data), 'source_bytes': size, 'truncated': size > len(data)})
                    if data:
                        archive.writestr('logs/'+path.name, data)
                        remaining -= len(data)
            except OSError as error:
                entry['unavailable'] = str(error)
            manifest['files'].append(entry)
        archive.writestr('daemon-state.json', json.dumps(daemon_state(), indent=2))
        binaries = []
        for name in ('speech-service', 'voice-dictation'):
            path = binary_dir/name
            try:
                with path.open('rb') as stream:
                    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
                binaries.append({'path': str(path), 'sha256': digest, 'bytes': path.stat().st_size})
            except OSError as error:
                binaries.append({'path': str(path), 'unavailable': str(error)})
        archive.writestr('binaries.json', json.dumps(binaries, indent=2))
        if system:
            commands = {
                'service': ['systemctl', '--user', 'show', 'speech-to-text-daemon.service', '--property=ActiveState,SubState,MainPID,Result,ExecMainCode,ExecMainStatus,LimitCORE,KillMode'],
                'journal': ['journalctl', '--user', '-u', 'speech-to-text-daemon.service', '--since=-7days', '-n', '20000', '--no-pager', '-o', 'short-precise'],
                'kernel': ['journalctl', '-k', '--since=-7days', '--no-pager', '-n', '2000', '--grep=speech-service|voice-dictation'],
            }
            for name in ('speech-service', 'voice-dictation'):
                commands['coredumps-'+name] = ['coredumpctl', '--no-pager', '--since=-7days', 'list', str(binary_dir/name)]
                commands['elf-'+name] = ['readelf', '-n', str(binary_dir/name)]
            for name, args in commands.items():
                archive.writestr(name+'.json', json.dumps(command(args), indent=2))
            try:
                archive.writestr('core-pattern.txt', Path('/proc/sys/kernel/core_pattern').read_text())
            except OSError as error:
                manifest['core_pattern_unavailable'] = str(error)
        archive.writestr('manifest.json', json.dumps(manifest, indent=2))
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--log-path', type=Path, default=Path.home()/'.local/share/speech-to-text/app.log')
    parser.add_argument('--binary-dir', type=Path, default=Path(__file__).resolve().parents[1]/'target/release')
    args = parser.parse_args()
    output = args.output or Path.home()/'.local/share/speech-to-text/diagnostics'/f'report-{time.time_ns()}.zip'
    collect(output, args.log_path, args.binary_dir)
    print(json.dumps({'path': str(output.absolute()), 'bytes': output.stat().st_size}))


if __name__ == '__main__':
    main()
