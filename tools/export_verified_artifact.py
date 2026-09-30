#!/usr/bin/env python3
"""Copy a passed artifact pair into a portable bundle, preserving build provenance."""
import argparse
import json
from pathlib import Path
import shutil
from verify import artifact_hashes, write_report


def export(manifest_path, destination):
    manifest = json.loads(manifest_path.read_text())
    if manifest.get('verification', {}).get('status') != 'passed' or manifest['verification'].get('mode') != 'full':
        raise ValueError('Only a passed full gate can be exported')
    artifact = Path(manifest['artifact_dir'])
    if not artifact.is_absolute():
        artifact = manifest_path.parent/artifact
    if artifact_hashes(artifact) != manifest['binary_sha256']:
        raise ValueError('Artifact hashes changed after verification')
    destination.mkdir(parents=True, exist_ok=False)
    binaries = destination/'bin'
    binaries.mkdir()
    for name in ('voice-dictation', 'speech-service'):
        shutil.copy2(artifact/name, binaries/name)
    if artifact_hashes(binaries) != manifest['binary_sha256']:
        raise ValueError('Exported pair hash mismatch')
    manifest['artifact_dir'] = 'bin'
    manifest['binary'] = 'bin/voice-dictation'
    write_report(destination/'manifest.json', manifest)
    return destination/'manifest.json'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    print(export(args.manifest.resolve(), args.destination.resolve()))


if __name__ == '__main__':
    main()
