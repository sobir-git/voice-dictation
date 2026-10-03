#!/usr/bin/env python3
"""Immutable pair installation and bounded causal diagnostics (standard library only)."""
import argparse
from contextlib import contextmanager
import fcntl
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

from verify import artifact_hashes, now, source_fingerprint
import native_cpu

NAMES = ('voice-dictation', 'speech-service')


def data_dir():
    return Path(os.environ.get('XDG_DATA_HOME', str(Path.home()/'.local/share')))/'speech-to-text'


@contextmanager
def locked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as stream:
        os.chmod(path, 0o600)
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def event(stage, outcome='passed', **fields):
    directory = data_dir()
    with locked(directory/'installation-log.lock'):
        path = directory/'installation.jsonl'
        if path.exists() and path.stat().st_size >= 1024*1024:
            for index in range(4, 0, -1):
                older = Path(str(path)+f'.{index}')
                if older.exists():
                    older.replace(Path(str(path)+f'.{index+1}'))
            path.replace(Path(str(path)+'.1'))
        with path.open('a') as stream:
            os.chmod(path, 0o600)
            stream.write(json.dumps(dict(schema=1, time=now(), pid=os.getpid(), stage=stage,
                                        outcome=outcome, **fields), sort_keys=True)+'\n')


def capability(directory):
    for name in NAMES:
        path = directory/name
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f'Missing executable pair member: {path}')
    result = subprocess.run([str(directory/'speech-service'), '--list-optimizations'],
                            check=True, capture_output=True, text=True, timeout=15)
    value = json.loads(result.stdout)['host']['vulkan_build']
    if type(value) is not bool:
        raise ValueError('Invalid compiled backend capability')
    return 'vulkan' if value else 'cpu'


def current():
    path = data_dir()/'installations/current'
    if not path.is_symlink():
        if path.exists():
            raise ValueError('Installed current must be a release symlink')
        return None, None
    release = path.resolve(strict=True)
    if release.parent != (path.parent/'releases').resolve():
        raise ValueError('Installed release is outside the releases directory')
    receipt = json.loads((release/'receipt.json').read_text())
    return release, receipt


def selection(root, release, receipt):
    if 'VOICE_DICTATION_FEATURES' in os.environ:
        features = os.environ['VOICE_DICTATION_FEATURES'].strip()
        if features not in ('', 'vulkan'):
            raise ValueError('VOICE_DICTATION_FEATURES must be empty (CPU) or vulkan')
        return ('vulkan' if features else 'cpu'), 'explicit'
    if receipt is not None:
        native_cpu.require_host(native_cpu.provenance_isa(receipt['provenance']))
        if capability(release) != receipt['backend']:
            raise ValueError('Installed receipt disagrees with binary capability')
        if artifact_hashes(release) != receipt['binary_sha256']:
            raise ValueError('Installed pair hashes disagree with receipt')
        return receipt['backend'], 'preserved_receipt'
    legacy = root/'target/release'
    if all((legacy/name).is_file() for name in NAMES):
        return capability(legacy), 'legacy_migration'
    return 'cpu', 'fresh_cpu'


def verified_artifact(root, manifest_path, backend):
    manifest = json.loads(manifest_path.read_text())
    if manifest.get('verification', {}).get('status') != 'passed' or manifest['verification'].get('mode') != 'full':
        raise ValueError('Artifact requires passed full verification')
    if manifest.get('schema') != 2 or manifest.get('backend') != backend or manifest.get('features') != ([] if backend == 'cpu' else ['vulkan']):
        raise ValueError('Verified artifact edition/schema mismatch')
    # Source is portable. The complete original build provenance remains VM-specific.
    local = source_fingerprint(root, backend, Path(manifest['target_dir']))
    if manifest.get('source_digest') != local['source_digest']:
        raise ValueError('Verified artifact source is stale or mismatched')
    provenance = {key: manifest[key] for key in local}
    fingerprint = provenance.pop('fingerprint')
    if hashlib.sha256(json.dumps(provenance, sort_keys=True, separators=(',', ':')).encode()).hexdigest() != fingerprint:
        raise ValueError('Invalid verified build provenance fingerprint')
    artifact = Path(manifest['artifact_dir'])
    if not artifact.is_absolute():
        artifact = manifest_path.parent/artifact
    if artifact_hashes(artifact) != manifest['binary_sha256']:
        raise ValueError('Verified artifact pair hash mismatch')
    isa = native_cpu.provenance_isa(manifest)
    if native_cpu.ENV in os.environ and native_cpu.preset(os.environ[native_cpu.ENV]) != isa:
        raise ValueError('Verified artifact CPU ISA disagrees with explicit selection')
    # This check must precede capability(), which executes the candidate service.
    native_cpu.require_host(isa)
    return artifact, manifest


def install(root, verified=None):
    directory = data_dir()/'installations'
    with locked(directory/'install.lock'):
        install_id = uuid.uuid4().hex
        stage = 'selection'
        fields = dict(install_id=install_id)
        staging = None
        try:
            old, receipt = current()
            isa, isa_reason = native_cpu.selection(receipt, os.environ)
            backend, reason = selection(root, old, receipt)
            fields.update(backend=backend, selection_source=reason,
                          cpu_isa=isa, cpu_isa_selection_source=isa_reason,
                          old_backend=receipt['backend'] if receipt else 'historical_unknown',
                          previous_install_id=receipt['install_id'] if receipt else None)
            event(stage, **fields)
            stage = 'runtime_prerequisites'
            runtime_tools = {name: shutil.which(name) for name in ('ffmpeg', 'arecord')}
            missing = [name for name, path in runtime_tools.items() if path is None]
            if missing:
                raise ValueError('Missing recording runtime prerequisite(s): ' + ', '.join(missing))
            event(stage, tools=runtime_tools, **fields)
            stage = 'artifact_validation' if verified else 'build'
            if verified:
                artifact, provenance = verified_artifact(root, verified.resolve(), backend)
                isa = native_cpu.provenance_isa(provenance)
                fields.update(cpu_isa=isa, cpu_isa_selection_source='verified_artifact')
            else:
                environment = native_cpu.compose(os.environ, isa)
                native_cpu.require_host(isa)
                configured = Path(os.environ.get('CARGO_TARGET_DIR', str(root/'artifacts/installation/target')))
                if not configured.is_absolute():
                    configured = Path.cwd()/configured
                target = configured.resolve()/backend
                environment['CARGO_TARGET_DIR'] = str(target)
                provenance = source_fingerprint(root, backend, target, environment)
                command = ['cargo', 'build', '--manifest-path', str(root/'Cargo.toml'), '--locked', '--release']
                if backend == 'vulkan':
                    command += ['--features', 'vulkan']
                event(stage, 'started', target_dir=str(target), build_id=provenance['fingerprint'], **fields)
                subprocess.run(command, env=environment, cwd=root, check=True)
                if source_fingerprint(root, backend, target, environment) != provenance:
                    raise ValueError('Build inputs changed during compilation')
                artifact = target/'release'
            fields.update(build_id=provenance['fingerprint'], source_digest=provenance['source_digest'],
                          artifact_dir=str(artifact), toolchain=provenance['rustc'])
            if capability(artifact) != backend:
                raise ValueError('Built binary capability disagrees with selected edition')
            hashes = artifact_hashes(artifact)
            event(stage, binary_sha256=hashes, **fields)
            stage = 'publish'
            releases = directory/'releases'
            releases.mkdir(parents=True, exist_ok=True)
            staging = Path(tempfile.mkdtemp(prefix='.staging-', dir=releases))
            for name in NAMES:
                shutil.copyfile(artifact/name, staging/name)
                (staging/name).chmod(0o555)
            if artifact_hashes(staging) != hashes or capability(staging) != backend:
                raise ValueError('Staged pair changed during publication')
            record = dict(schema=1, install_id=install_id, backend=backend, features=provenance['features'],
                          cpu_isa=isa, cpu_isa_selection_source=fields['cpu_isa_selection_source'],
                          selection_source=reason, installed_at=now(), binary_sha256=hashes,
                          build_id=provenance['fingerprint'], provenance=provenance,
                          source_revision=subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=root, capture_output=True, text=True, check=True).stdout.strip())
            (staging/'receipt.json').write_text(json.dumps(record, indent=2)+'\n')
            (staging/'receipt.json').chmod(0o444)
            for name in (*NAMES, 'receipt.json'):
                with (staging/name).open('rb') as stream:
                    os.fsync(stream.fileno())
            release = releases/install_id
            staging.rename(release)
            staging = None
            release.chmod(0o555)
            link = directory/f'.current-{install_id}'
            link.symlink_to(Path('releases')/install_id)
            os.replace(link, directory/'current')
            descriptor = os.open(directory, os.O_RDONLY)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
            event('current_release_switch', old_release=str(old) if old else None,
                  new_release=str(release), binary_sha256=hashes, **fields)
            print(f'Installed {backend} pair: {release}')
        except Exception as error:
            event(stage, 'failed', error=str(error), **fields)
            raise
        finally:
            if staging:
                shutil.rmtree(staging)


def resolve(component='desktop'):
    try:
        release, receipt = current()
        if release is None or not all(os.access(release/name, os.X_OK) for name in NAMES):
            raise ValueError('No installed executable pair. Run ./install.sh (or ./update_local.sh) first.')
        native_cpu.require_host(native_cpu.provenance_isa(receipt['provenance']))
        event('launch', install_id=receipt['install_id'], build_id=receipt['build_id'],
              backend=receipt['backend'], release=str(release), component=component,
              executable=str(release/('speech-service' if component == 'daemon' else 'voice-dictation')))
        print(release)
    except Exception as error:
        event('launch', 'failed', component=component, error=str(error))
        raise


def restart(stage):
    release, receipt = current()
    if release is None:
        raise ValueError('No installed release for restart')
    deadline = time.monotonic() + (5 if stage == 'restart-result' else 0)
    while True:
        observed = subprocess.run(['systemctl', '--user', 'show', 'speech-to-text-daemon.service', '--property=MainPID', '--value'], capture_output=True, text=True, check=False).stdout.strip()
        executable = None
        if observed.isdigit() and observed != '0':
            try:
                executable = str(Path(f'/proc/{observed}/exe').resolve(strict=True))
            except OSError:
                pass
        matched = executable == str(release/'speech-service')
        if matched or time.monotonic() >= deadline:
            break
        time.sleep(0.1)
    outcome = ('passed' if matched else 'failed') if stage == 'restart-result' else ('failed' if stage == 'restart-failed' else 'started')
    event(stage, outcome, install_id=receipt['install_id'], build_id=receipt['build_id'],
          expected_release=str(release), observed_pid=observed,
          observed_executable=executable, observed_matches_release=matched)
    if stage == 'restart-result' and not matched:
        raise ValueError('Restarted daemon executable does not match the selected installed release')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('install', 'resolve', 'restart-start', 'restart-result', 'restart-failed'))
    parser.add_argument('--verified-artifact', type=Path)
    parser.add_argument('--component', choices=('desktop', 'daemon'), default='desktop')
    args = parser.parse_args()
    try:
        if args.action == 'install':
            install(Path(__file__).resolve().parents[1], args.verified_artifact)
        elif args.action == 'resolve':
            resolve(args.component)
        else:
            restart(args.action)
    except Exception as error:
        print(f'Installation: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
