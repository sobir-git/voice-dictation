"""Fixed CPU floor composition and Linux pre-execution compatibility."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tools'))
import native_cpu
import verify


class NativeCpuTests(unittest.TestCase):
    def test_unspecified_flags_unchanged_and_exact_measured_preset(self):
        original = {'CXXFLAGS': '-O2', 'TRANSCRIBE_CMAKE_ARGS': '-DGGML_NATIVE=ON'}
        self.assertEqual(native_cpu.compose(original), original)
        env = native_cpu.compose({}, 'avx2')
        self.assertEqual(env['TRANSCRIBE_CMAKE_ARGS'], native_cpu.AVX2_CMAKE_ARGS)
        self.assertEqual(native_cpu.compose(env), env)
        self.assertNotIn('-march', env['TRANSCRIBE_CMAKE_ARGS'])
        self.assertTrue(all(v == 'OFF' for k, v in native_cpu.DEFINES.items()
                            if 'AVX512' in k or 'VNNI' in k))

    def test_unrelated_cmake_options_preserved_with_both_inputs(self):
        env = native_cpu.compose({'CMAKE_ARGS': '-DTRANSCRIBE_BUILD_TESTS=OFF',
                                  'TRANSCRIBE_CMAKE_ARGS': '-DCUSTOM="two words" -DGGML_NATIVE=OFF'}, 'avx2')
        self.assertEqual(env['CMAKE_ARGS'], '-DTRANSCRIBE_BUILD_TESTS=OFF')
        self.assertEqual(env['TRANSCRIBE_CMAKE_ARGS'], native_cpu.AVX2_CMAKE_ARGS + ' -DCUSTOM="two words"')

    def test_conflicting_and_unknown_presets_rejected(self):
        for env in ({'CMAKE_ARGS': '-DGGML_AVX2=OFF'}, {'TRANSCRIBE_CMAKE_ARGS': '-DGGML_NATIVE=ON'},
                    {'CMAKE_ARGS': '-DGGML_AVX512_VL=ON'}, {'CMAKE_ARGS': '-DGGML_AVX_VNNI=ON'},
                    {'CFLAGS': '-march=native'}, {'CXXFLAGS_x86_64_unknown_linux_gnu': '-mavx512f'},
                    {'RUSTFLAGS': '-C target-cpu=haswell'}, {'CARGO_ENCODED_RUSTFLAGS': '-C\x1ftarget-feature=+avxvnni'},
                    {'CXX': 'c++ -march=skylake'}, {'TRANSCRIBE_DIR': '/unverified'},
                    {'CARGO_BUILD_TARGET': 'aarch64-unknown-linux-gnu'}):
            with self.subTest(env=env), self.assertRaises(ValueError):
                native_cpu.compose(env, 'avx2')
        with self.assertRaises(ValueError):
            native_cpu.compose({}, 'native')

    def test_host_floor_checks_every_cpu_and_os_exposed_avx(self):
        flags = ' '.join(native_cpu.AVX2_FLOOR)
        with patch.object(native_cpu.platform, 'system', return_value='Linux'), \
                patch.object(native_cpu.platform, 'machine', return_value='x86_64'), \
                patch.object(Path, 'read_text', return_value='flags : ' + flags + '\nflags : ' + flags):
            native_cpu.require_host('avx2')
        for missing in native_cpu.AVX2_FLOOR:
            second = ' '.join(f for f in native_cpu.AVX2_FLOOR if f != missing)
            with self.subTest(missing=missing), patch.object(native_cpu.platform, 'system', return_value='Linux'), \
                    patch.object(native_cpu.platform, 'machine', return_value='x86_64'), \
                    patch.object(Path, 'read_text', return_value='flags : ' + flags + '\nflags : ' + second):
                with self.assertRaisesRegex(ValueError, missing):
                    native_cpu.require_host('avx2')
        with patch.object(native_cpu.platform, 'machine', return_value='aarch64'):
            with self.assertRaisesRegex(ValueError, 'x86-64'):
                native_cpu.require_host('avx2')

    def test_preset_changes_cache_fingerprint_not_source_digest(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(verify, 'git_inputs', return_value=[]), \
                patch.object(verify, 'command_output', return_value='version'):
            root = Path(tmp)
            base = verify.source_fingerprint(root, 'vulkan', root / 'target', {})
            avx = verify.source_fingerprint(root, 'vulkan', root / 'target', native_cpu.compose({}, 'avx2'))
            self.assertEqual(base['schema'], 2)
            self.assertEqual(base['source_digest'], avx['source_digest'])
            self.assertNotEqual(base['fingerprint'], avx['fingerprint'])
            self.assertIn(native_cpu.ENV, avx['environment'])
            self.assertEqual(avx['build_flags']['cpu_isa_floor'], list(native_cpu.AVX2_FLOOR))

    def test_malformed_provenance_floor_rejected(self):
        with self.assertRaisesRegex(ValueError, 'floor'):
            native_cpu.provenance_isa({'build_flags': {'cpu_isa': 'avx2'}})
