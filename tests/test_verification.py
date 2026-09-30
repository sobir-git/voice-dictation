"""Portable source and complete build provenance contracts."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'tools'))
import verify


class ProvenanceTests(unittest.TestCase):
    def test_toolchain_env_features_target_and_source_change_fingerprint(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root/'source.rs').write_text('first')
            with patch.object(verify, 'git_inputs', return_value=[Path('source.rs')]), patch.object(verify, 'command_output', return_value='version 1'):
                original = verify.source_fingerprint(root, 'cpu', root/'cpu')
                other_backend = verify.source_fingerprint(root, 'vulkan', root/'cpu')
                other_target = verify.source_fingerprint(root, 'cpu', root/'elsewhere')
                with patch.dict(os.environ, RUSTFLAGS='-C target-cpu=native'):
                    other_env = verify.source_fingerprint(root, 'cpu', root/'cpu')
                with patch.dict(os.environ, CMAKE_ARGS='-DGGML_NATIVE=OFF'):
                    fallback_env = verify.source_fingerprint(root, 'cpu', root/'cpu')
                with patch.object(verify, 'command_output', return_value='version 2'):
                    other_toolchain = verify.source_fingerprint(root, 'cpu', root/'cpu')
                for changed in (other_backend, other_target, other_env, fallback_env, other_toolchain):
                    self.assertNotEqual(original['fingerprint'], changed['fingerprint'])
                    self.assertEqual(original['source_digest'], changed['source_digest'])
                (root/'source.rs').write_text('dirty edit')
                changed = verify.source_fingerprint(root, 'cpu', root/'cpu')
                self.assertNotEqual(original['source_digest'], changed['source_digest'])
                self.assertNotIn('-C target-cpu=native', str(other_env))

    def test_native_probe_has_backend_specific_output(self):
        steps = verify.mode_steps('full', 'vulkan', Path('/fixture/vulkan/voice-dictation'))
        self.assertEqual(steps[-1][1][-2:], ['--output', '/fixture/vulkan/native'])

    def test_effective_cc_cxx_commands_are_queried(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(verify, 'git_inputs', return_value=[]), patch.object(verify, 'command_output', return_value='tool') as output, patch.dict(os.environ, CC='custom-cc --flag', CXX='custom-cxx'):
            verify.source_fingerprint(Path(tmp), 'cpu', Path(tmp)/'target')
            commands = [call.args[0] for call in output.call_args_list]
            self.assertIn(['custom-cc', '--flag', '--version'], commands)
            self.assertIn(['custom-cxx', '--version'], commands)

    def test_manifest_only_passes_after_probe_and_failed_retry_revokes_gate(self):
        import json
        import subprocess
        for failed_step in (None, 'native-probe', 'cargo-test'):
            with self.subTest(failed_step=failed_step), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                target = root/'artifacts/verification/target/cpu/release'
                target.mkdir(parents=True)
                for name in ('voice-dictation', 'speech-service'):
                    (target/name).write_text('synthetic artifact')
                provenance = {'schema':2, 'fingerprint':'complete', 'source_digest':'source'}
                manifest_path = root/'artifacts/verification/provenance-cpu.json'
                manifest_path.write_text(json.dumps(dict(provenance, verification={'status':'passed', 'mode':'full'})))
                steps = [('cargo-test', ['fake-test'], 5), ('release-build', ['fake-build'], 5), ('native-probe', ['fake-probe'], 5)]
                def run(command, **kwargs):
                    stage = {'fake-test':'cargo-test', 'fake-build':'release-build', 'fake-probe':'native-probe'}[command[0]]
                    if stage == 'native-probe':
                        self.assertEqual(json.loads(manifest_path.read_text())['verification']['status'], 'built')
                    return subprocess.CompletedProcess(command, 1 if stage == failed_step else 0)
                with patch.object(verify, '__file__', str(root/'tools/verify.py')), patch.object(sys, 'argv', ['verify.py', 'full']), patch.dict(os.environ, {}, clear=True), patch.object(verify, 'source_fingerprint', return_value=provenance), patch.object(verify, 'mode_steps', return_value=steps), patch.object(verify.subprocess, 'run', side_effect=run):
                    result = verify.main()
                status = json.loads(manifest_path.read_text())['verification']['status']
                self.assertEqual(result, 0 if failed_step is None else 1)
                self.assertEqual(status, {None:'passed', 'native-probe':'built', 'cargo-test':'pending'}[failed_step])

    def test_cold_runner_creates_log_directory_before_first_command(self):
        import subprocess
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with patch.object(verify, '__file__', str(root/'tools/verify.py')), patch.object(sys, 'argv', ['verify.py', 'full']), patch.dict(os.environ, {}, clear=True), patch.object(verify, 'source_fingerprint', return_value={}), patch.object(verify, 'mode_steps', return_value=[('cargo-test', ['fixture'], 1)]), patch.object(verify.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)) as run:
                self.assertEqual(verify.main(), 1)
                run.assert_called_once()
            self.assertEqual(len(list((root/'artifacts/verification').glob('*cargo-test.log'))), 1)
