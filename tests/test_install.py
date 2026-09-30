"""Installer contracts, using fake builds and an isolated home."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class InstallTests(unittest.TestCase):
    def run_install(self, installed_gpu, explicit=None):
        with tempfile.TemporaryDirectory(prefix='voice-install-') as temporary:
            root = Path(temporary)
            project = root / 'project'
            project.mkdir()
            shutil.copy2(Path(__file__).resolve().parents[1] / 'install.sh', project)
            (project / 'Cargo.toml').write_text('')
            (project / 'install_icons.sh').write_text('#!/bin/sh\nexit 0\n')
            (project / 'install_icons.sh').chmod(0o755)
            installed = project / 'target/release'
            installed.mkdir(parents=True)
            service = installed / 'speech-service'
            service.write_text('#!/bin/sh\necho \'{"host":{"vulkan_build": ' + str(installed_gpu).lower() + '}}\'\n')
            service.chmod(0o755)
            home = root / 'home'
            config = home / '.config/speech-to-text/config.yaml'
            config.parent.mkdir(parents=True)
            config.write_text('preserve this exact configuration\n')
            fakebin = root / 'bin'
            fakebin.mkdir()
            for name in ['cmake', 'ffmpeg', 'arecord']:
                path = fakebin / name
                path.write_text('#!/bin/sh\nexit 0\n')
                path.chmod(0o755)
            cargo = fakebin / 'cargo'
            cargo.write_text('''#!/bin/sh
printf '%s\\n' "$@" > "$TEST_BUILD_ARGS"
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/bin/sh\\necho verified-desktop\\n' > "$CARGO_TARGET_DIR/release/voice-dictation"
printf '#!/bin/sh\\necho verified-service\\n' > "$CARGO_TARGET_DIR/release/speech-service"
''')
            cargo.chmod(0o755)
            args = root / 'args'
            env = {**os.environ, 'HOME': str(home), 'PATH': str(fakebin) + ':' + os.environ['PATH'],
                   'CARGO_TARGET_DIR': str(root / 'verified'), 'TEST_BUILD_ARGS': str(args)}
            env.pop('VOICE_DICTATION_FEATURES', None)
            if explicit is not None:
                env['VOICE_DICTATION_FEATURES'] = explicit
            subprocess.run(['bash', str(project / 'install.sh')], env=env, check=True, capture_output=True)
            self.assertEqual(config.read_text(), 'preserve this exact configuration\n')
            self.assertEqual(subprocess.check_output([str(service)], text=True).strip(), 'verified-service')
            self.assertEqual(subprocess.check_output([str(installed / 'voice-dictation')], text=True).strip(), 'verified-desktop')
            return args.read_text().splitlines()

    def test_direct_install_preserves_vulkan_and_installs_verified_target(self):
        self.assertEqual(self.run_install(True)[-2:], ['--features', 'vulkan'])

    def test_explicit_cpu_override_is_respected(self):
        self.assertNotIn('--features', self.run_install(True, ''))

    def test_cpu_stays_cpu_until_vulkan_is_requested(self):
        self.assertNotIn('--features', self.run_install(False))
        self.assertEqual(self.run_install(False, 'vulkan')[-2:], ['--features', 'vulkan'])
