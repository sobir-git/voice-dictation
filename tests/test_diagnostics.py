import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location('diagnostics', Path(__file__).resolve().parents[1]/'tools/diagnostics.py')
diagnostics = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostics)


class DiagnosticsTests(unittest.TestCase):
    def test_export_retains_rotated_logs_and_reports_truncation_without_user_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root/'app.log').write_text('new\n')
            (root/'app.log.1').write_text('old\n')
            (root/'hud-123.log').write_text('paint\n')
            (root/'history.db').write_text('private transcript')
            (root/'recording.wav').write_text('private audio')
            output = root/'report.zip'
            with patch.object(diagnostics, 'daemon_state', return_value={'unavailable': 'offline'}), patch.object(diagnostics, 'FILE_LIMIT', 4):
                manifest = diagnostics.collect(output, root/'app.log', root, system=False)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            with zipfile.ZipFile(output) as archive:
                self.assertIn('logs/app.log.1', archive.namelist())
                self.assertIn('logs/hud-123.log', archive.namelist())
                self.assertFalse(any('history' in name or '.wav' in name for name in archive.namelist()))
                self.assertEqual(json.loads(archive.read('daemon-state.json'))['unavailable'], 'offline')
            self.assertTrue(any(row.get('truncated') for row in manifest['files']))
            with self.assertRaises(FileExistsError):
                diagnostics.collect(output, root/'app.log', root, system=False)

    def test_missing_system_tool_is_explicit(self):
        result = diagnostics.command(['/nonexistent/voice-diagnostics-command'])
        self.assertIn('unavailable', result)
