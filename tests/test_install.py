"""Installation boundary contracts with temporary homes and synthetic executable pairs."""
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools'))
import installation
import verify
import export_verified_artifact
import native_cpu


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='voice-install-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root/'project'
        self.project.mkdir()
        for name in ('run.sh', 'install.sh', 'config.yaml.example', '.gitignore'):
            shutil.copy2(ROOT/name, self.project/name)
        (self.project/'tools').mkdir()
        for name in ('installation.py', 'verify.py', 'native_cpu.py'):
            shutil.copy2(ROOT/'tools'/name, self.project/'tools'/name)
        (self.project/'install_icons.sh').write_text('#!/bin/sh\nexit 0\n')
        (self.project/'install_icons.sh').chmod(0o755)
        (self.project/'Cargo.toml').write_text('synthetic build source')
        subprocess.run(['git', 'init', '-q', str(self.project)], check=True)
        subprocess.run(['git', 'add', '.'], cwd=self.project, check=True)
        subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture'], cwd=self.project, check=True)
        self.home = self.root/'home'
        self.config = self.home/'.config/speech-to-text/config.yaml'
        self.config.parent.mkdir(parents=True)
        self.config.write_text('keep private config exactly\n')
        self.data = self.home/'.local/share/speech-to-text'
        self.data.mkdir(parents=True)
        (self.data/'history.db').write_text('private history')
        (self.data/'models').mkdir()
        (self.data/'models/cache').write_text('model data')
        self.bin = self.root/'bin'
        self.bin.mkdir()
        for name in ('ffmpeg', 'arecord'):
            (self.bin/name).write_text('#!/bin/sh\nexit 0\n')
            (self.bin/name).chmod(0o755)
        cargo = self.bin/'cargo'
        cargo.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
if sys.argv[1:] == ['-V']:
    print('fixture cargo'); sys.exit(0)
with open(os.environ['TEST_BUILD_ARGS'], 'w') as f: json.dump(sys.argv[1:], f)
if os.environ.get('TEST_FAIL'): sys.exit(13)
out = pathlib.Path(os.environ['CARGO_TARGET_DIR'])/'release'
out.mkdir(parents=True, exist_ok=True)
gpu = '--features' in sys.argv
if os.environ.get('TEST_MISMATCH'): gpu = not gpu
for name in ['voice-dictation', 'speech-service']:
    if os.environ.get('TEST_MISSING') == name:
        (out/name).unlink(missing_ok=True); continue
    script = '#!/bin/sh\\n'
    if name == 'speech-service':
        script += "echo '" + json.dumps({'host':{'vulkan_build':gpu}}) + "'\\n"
    else:
        script += 'printf "%s\\\\n" "$0"\\n'
    (out/name).write_text(script)
    (out/name).chmod(0o755)
''')
        cargo.chmod(0o755)
        (self.bin/'id').write_text('#!/bin/sh\necho input\n')
        (self.bin/'id').chmod(0o755)
        self.environment = {**os.environ, 'HOME': str(self.home),
                            'PATH': str(self.bin)+':'+os.environ['PATH'],
                            'CARGO_TARGET_DIR': str(self.root/'builds'),
                            'TEST_BUILD_ARGS': str(self.root/'args'),
                            'XDG_RUNTIME_DIR': str(self.root/'runtime'),
                            'PRIVATE_TOKEN': 'must-not-appear'}
        for key in ('VOICE_DICTATION_FEATURES', 'VOICE_DICTATION_CPU_ISA', 'TRANSCRIBE_CMAKE_ARGS',
                    'CMAKE_ARGS', 'XDG_DATA_HOME', 'TEST_FAIL', 'TEST_MISSING', 'TEST_MISMATCH'):
            self.environment.pop(key, None)
        self.env_patch = patch.dict(os.environ, self.environment, clear=True)
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)

    def install(self, explicit=None, **env):
        if explicit is None:
            os.environ.pop('VOICE_DICTATION_FEATURES', None)
        else:
            os.environ['VOICE_DICTATION_FEATURES'] = explicit
        with patch.dict(os.environ, env):
            installation.install(self.project)
        self.assertEqual(self.config.read_text(), 'keep private config exactly\n')
        self.assertEqual((self.data/'history.db').read_text(), 'private history')
        self.assertEqual((self.data/'models/cache').read_text(), 'model data')
        return installation.current()

    def pair(self, directory, gpu=False):
        directory.mkdir(parents=True, exist_ok=True)
        for name in installation.NAMES:
            (directory/name).write_text('#!/bin/sh\necho \'{"host":{"vulkan_build": '+str(gpu).lower()+'}}\'\n')
            (directory/name).chmod(0o755)

    def test_fresh_cpu_explicit_gpu_cpu_and_sticky_receipt(self):
        release, receipt = self.install()
        self.assertEqual(receipt['selection_source'], 'fresh_cpu')
        self.assertEqual(receipt['backend'], 'cpu')
        gpu_release, receipt = self.install('vulkan')
        self.assertEqual(receipt['backend'], 'vulkan')
        self.assertEqual(receipt['selection_source'], 'explicit')
        self.pair(self.project/'target/release', gpu=False)
        _, receipt = self.install()
        self.assertEqual(receipt['backend'], 'vulkan')
        self.assertEqual(receipt['selection_source'], 'preserved_receipt')
        shutil.rmtree(self.project/'target')
        _, receipt = self.install()
        self.assertEqual(receipt['backend'], 'vulkan')
        _, receipt = self.install('')
        self.assertEqual(receipt['backend'], 'cpu')
        self.assertEqual(installation.capability(gpu_release), 'vulkan')
        self.assertTrue(release.exists())

    def test_legacy_migration_only_when_no_receipt(self):
        self.pair(self.project/'target/release', gpu=True)
        _, receipt = self.install()
        self.assertEqual(receipt['backend'], 'vulkan')
        self.assertEqual(receipt['selection_source'], 'legacy_migration')

    def test_build_isolated_and_failures_keep_pair_and_receipt(self):
        old, receipt = self.install('vulkan')
        for env in ({'TEST_FAIL':'1'}, {'TEST_MISSING':'voice-dictation'}, {'TEST_MISSING':'speech-service'}, {'TEST_MISMATCH':'1'}):
            with self.subTest(env=env), self.assertRaises((ValueError, subprocess.CalledProcessError)):
                self.install('', **env)
            self.assertEqual(installation.current(), (old, receipt))
        self.assertEqual(receipt['provenance']['target_dir'], str(self.root/'builds/vulkan'))
        self.assertFalse((self.project/'target').exists())
        records = [json.loads(line) for line in (self.data/'installation.jsonl').read_text().splitlines()]
        self.assertEqual(sum(r['outcome']=='failed' for r in records), 4)
        self.assertIn('binary_sha256', next(r for r in records if r['stage']=='current_release_switch'))
        self.assertNotIn('must-not-appear', json.dumps(records))
        self.assertNotIn('private config', json.dumps(records))

    def test_receipt_capability_mismatch_refuses_preservation(self):
        release, receipt = self.install('vulkan')
        (release/'speech-service').chmod(0o755)
        (release/'speech-service').write_text('#!/bin/sh\necho \'{"host":{"vulkan_build":false}}\'\n')
        with self.assertRaisesRegex(ValueError, 'capability'):
            self.install()
        self.assertEqual(installation.current(), (release, receipt))

    def manifest(self, backend='vulkan'):
        artifact = self.root/'verified-pair'
        self.pair(artifact, backend=='vulkan')
        provenance = verify.source_fingerprint(self.project, backend, self.root/'vm-build')
        record = dict(provenance, artifact_dir=str(artifact), binary_sha256=verify.artifact_hashes(artifact),
                      verification={'status':'passed', 'mode':'full'})
        path = self.root/'verified.json'
        path.write_text(json.dumps(record))
        return path, record

    def test_verified_install_no_build_and_portable_source(self):
        path, manifest = self.manifest()
        os.environ['VOICE_DICTATION_FEATURES'] = 'vulkan'
        with patch.object(installation, 'source_fingerprint', wraps=verify.source_fingerprint):
            installation.install(self.project, path)
        release, receipt = installation.current()
        self.assertEqual(receipt['provenance']['target_dir'], str(self.root/'vm-build'))
        self.assertFalse((self.root/'args').exists())
        self.assertEqual(verify.artifact_hashes(release), manifest['binary_sha256'])

    def test_stale_unverified_mismatched_missing_pair_refused(self):
        old, receipt = self.install('vulkan')
        path, original = self.manifest()
        for change in ({'source_digest':'stale'}, {'backend':'cpu'}, {'verification':{'status':'built', 'mode':'full'}}, {'fingerprint':'invalid'}):
            path.write_text(json.dumps(dict(original, **change)))
            with self.subTest(change=change), self.assertRaises(ValueError):
                installation.install(self.project, path)
            self.assertEqual(installation.current(), (old, receipt))
        path.write_text(json.dumps(original))
        (Path(original['artifact_dir'])/'speech-service').unlink()
        with self.assertRaises(FileNotFoundError):
            installation.install(self.project, path)
        self.assertEqual(installation.current(), (old, receipt))

    def test_launch_uses_canonical_installed_pair_despite_target_cpu(self):
        release, receipt = self.install('vulkan')
        self.pair(self.project/'target/release', False)
        result = subprocess.run(['bash', str(self.project/'run.sh')], check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout.strip(), str(release/'voice-dictation'))
        self.assertEqual(installation.capability(Path(result.stdout.strip()).parent), 'vulkan')
        record = json.loads((self.data/'installation.jsonl').read_text().splitlines()[-1])
        self.assertEqual(record['install_id'], receipt['install_id'])
        self.assertEqual(record['component'], 'desktop')
        self.assertEqual(record['executable'], str(release/'voice-dictation'))
        subprocess.run(['bash', str(self.project/'run.sh'), '--daemon', '--private-argument'], check=True, capture_output=True)
        record = json.loads((self.data/'installation.jsonl').read_text().splitlines()[-1])
        self.assertEqual(record['component'], 'daemon')
        self.assertEqual(record['executable'], str(release/'speech-service'))
        self.assertNotIn('private-argument', json.dumps(record))

    def test_absent_install_fails_without_build(self):
        self.pair(self.project/'target/release')
        result = subprocess.run(['bash', str(self.project/'run.sh')], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Run ./install.sh', result.stderr)
        self.assertFalse((self.root/'args').exists())
        self.assertEqual(json.loads((self.data/'installation.jsonl').read_text().splitlines()[-1])['outcome'], 'failed')

    def test_direct_install_integration_forwards_verified_option(self):
        path, _ = self.manifest()
        environment = dict(os.environ, VOICE_DICTATION_FEATURES='vulkan')
        subprocess.run(['bash', str(self.project/'install.sh'), '--verified-artifact', str(path)], env=environment, check=True, capture_output=True)
        self.assertEqual(installation.current()[1]['backend'], 'vulkan')
        self.assertFalse((self.root/'args').exists())

    def test_log_rotation_is_bounded(self):
        for _ in range(8):
            installation.event('fixture', payload='x'*(1024*1024))
        self.assertTrue((self.data/'installation.jsonl.5').exists())
        self.assertFalse((self.data/'installation.jsonl.6').exists())

    def test_restart_result_requires_exact_installed_executable(self):
        release, receipt = self.install('vulkan')
        result = subprocess.CompletedProcess([], 0, stdout='99999999\n')
        with patch.object(installation.subprocess, 'run', return_value=result), patch.object(installation.time, 'monotonic', side_effect=[0, 6]):
            with self.assertRaisesRegex(ValueError, 'does not match'):
                installation.restart('restart-result')
        self.assertEqual(json.loads((self.data/'installation.jsonl').read_text().splitlines()[-1])['outcome'], 'failed')
        with patch.object(installation.subprocess, 'run', return_value=result), patch.object(Path, 'resolve', return_value=release/'speech-service'), patch.object(installation, 'current', return_value=(release, receipt)):
            installation.restart('restart-result')
        self.assertEqual(json.loads((self.data/'installation.jsonl').read_text().splitlines()[-1])['outcome'], 'passed')

    def test_export_bundle_is_relocatable_and_preserves_provenance(self):
        path, manifest = self.manifest()
        bundle = self.root/'export'
        exported = export_verified_artifact.export(path, bundle)
        moved = self.root/'relocated'
        bundle.rename(moved)
        os.environ['VOICE_DICTATION_FEATURES'] = 'vulkan'
        installation.install(self.project, moved/exported.name)
        receipt = installation.current()[1]
        self.assertEqual(receipt['build_id'], manifest['fingerprint'])
        self.assertEqual(receipt['provenance']['artifact_dir'], 'bin')

    def test_input_group_launcher_executes_selected_pair_through_shell(self):
        release, _ = self.install('vulkan')
        (self.bin/'id').write_text('#!/bin/sh\necho dev\n')
        sg = self.bin/'sg'
        sg.write_text('#!/bin/sh\nprintf "%s" "$3" > "$TEST_SG_COMMAND"\nexec /bin/sh -c "$3"\n')
        sg.chmod(0o755)
        capture = self.root/'sg-command'
        with patch.dict(os.environ, TEST_SG_COMMAND=str(capture)):
            result = subprocess.run(['bash', str(self.project/'run.sh')], check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout.strip(), str(release/'voice-dictation'))
        self.assertTrue(capture.read_text().startswith('exec '))

    def test_missing_runtime_keeps_prior_release_before_build_or_verified_publish(self):
        old, receipt = self.install('vulkan')
        arguments = (self.root/'args').read_bytes()
        manifest, _ = self.manifest()
        (self.bin/'arecord').unlink()
        with patch.dict(os.environ, PATH=str(self.bin)):
            for verified in (None, manifest):
                with self.subTest(verified=verified), self.assertRaisesRegex(ValueError, 'arecord'):
                    installation.install(self.project, verified)
                self.assertEqual(installation.current(), (old, receipt))
        self.assertEqual((self.root/'args').read_bytes(), arguments)
        record = json.loads((self.data/'installation.jsonl').read_text().splitlines()[-1])
        self.assertEqual(record['stage'], 'runtime_prerequisites')
        self.assertEqual(record['outcome'], 'failed')

    def test_source_install_preserves_isa_and_explicit_default_resets_it(self):
        with patch.object(native_cpu, 'require_host'):
            _, receipt = self.install('vulkan', VOICE_DICTATION_CPU_ISA='avx2')
            self.assertEqual(receipt['provenance']['build_flags']['cpu_isa'], 'avx2')
            self.assertEqual(receipt['provenance']['environment']['TRANSCRIBE_CMAKE_ARGS'],
                             hashlib.sha256(native_cpu.AVX2_CMAKE_ARGS.encode()).hexdigest())
            _, receipt = self.install()
            self.assertEqual(receipt['backend'], 'vulkan')
            self.assertEqual(receipt['cpu_isa'], 'avx2')
            self.assertEqual(receipt['cpu_isa_selection_source'], 'preserved_receipt')
            _, receipt = self.install(VOICE_DICTATION_CPU_ISA='default')
            self.assertEqual(receipt['cpu_isa'], 'default')
            self.assertEqual(receipt['backend'], 'vulkan')

    def test_verified_avx2_incompatible_host_never_executes_candidate(self):
        old, old_receipt = self.install('vulkan')
        with patch.dict(os.environ, native_cpu.compose(os.environ, 'avx2'), clear=True):
            path, manifest = self.manifest()
        with patch.object(native_cpu, 'require_host', side_effect=ValueError('Incompatible AVX2 CPU/OS')), \
                patch.object(installation, 'capability') as capability:
            with self.assertRaisesRegex(ValueError, 'Incompatible AVX2'):
                installation.install(self.project, path)
            capability.assert_not_called()
        self.assertEqual(installation.current(), (old, old_receipt))

    def test_verified_avx2_receipt_is_preserved_without_install_build(self):
        with patch.dict(os.environ, native_cpu.compose(os.environ, 'avx2'), clear=True):
            path, manifest = self.manifest()
        os.environ['VOICE_DICTATION_FEATURES'] = 'vulkan'
        with patch.object(native_cpu, 'require_host') as check:
            installation.install(self.project, path)
            check.assert_called_with('avx2')
            self.assertEqual(installation.current()[1]['cpu_isa'], 'avx2')
            self.assertFalse((self.root/'args').exists())
            _, receipt = self.install()
            self.assertEqual(receipt['cpu_isa'], 'avx2')

    def test_verified_isa_explicit_mismatch_is_refused(self):
        with patch.dict(os.environ, native_cpu.compose(os.environ, 'avx2'), clear=True):
            path, _ = self.manifest()
        with patch.dict(os.environ, VOICE_DICTATION_FEATURES='vulkan', VOICE_DICTATION_CPU_ISA='default'):
            with self.assertRaisesRegex(ValueError, 'CPU ISA disagrees'):
                installation.install(self.project, path)
