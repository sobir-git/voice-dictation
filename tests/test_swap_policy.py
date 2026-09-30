"""Run service setup/update against fake systemd, including unsupported controls."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SwapPolicyTests(unittest.TestCase):
    def test_fresh_and_existing_service_without_swap_controller(self):
        with tempfile.TemporaryDirectory(prefix='voice-swap-policy-') as directory:
            root = Path(directory)
            project = root / 'project'
            project.mkdir()
            home = root / 'home'
            home.mkdir()
            commands = root / 'commands'
            commands.mkdir()
            for name in ('setup_autostart.sh', 'update_local.sh'):
                shutil.copy2(ROOT / name, project / name)
            (project / 'install.sh').write_text('#!/bin/sh\nexit 0\n')
            (project / 'install.sh').chmod(0o755)
            (project / 'tools').mkdir()
            (project / 'tools/installation.py').write_text('pass\n')
            for name, script in {
                'systemctl': '#!/bin/sh\nexit 0\n',
                'systemd-analyze': '#!/bin/sh\necho "Swap controller unavailable; resource control ignored" >&2\nexit 0\n',
                'pgrep': '#!/bin/sh\nexit 1\n',
            }.items():
                path = commands / name
                path.write_text(script)
                path.chmod(0o755)
            config = home / '.config/speech-to-text/config.yaml'
            config.parent.mkdir(parents=True)
            config.write_text('private config sentinel\n')
            env = {**os.environ, 'HOME': str(home), 'PATH': str(commands) + ':' + os.environ['PATH']}
            subprocess.run(['bash', str(project / 'setup_autostart.sh')], env=env, check=True, capture_output=True)
            unit = home / '.config/systemd/user/speech-to-text-daemon.service'
            self.assertIn('MemorySwapMax=0\n', unit.read_text())
            subprocess.run(['bash', str(project / 'update_local.sh')], env=env, check=True, capture_output=True)
            dropin = home / '.config/systemd/user/speech-to-text-daemon.service.d/worker-lifecycle.conf'
            self.assertIn('MemorySwapMax=0\n', dropin.read_text())
            for text in (unit.read_text(), dropin.read_text()):
                self.assertNotIn('MemoryMax=', text)
                self.assertNotIn('MemoryLow=', text)
            self.assertEqual(config.read_text(), 'private config sentinel\n')
