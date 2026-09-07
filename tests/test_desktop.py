import os
import tempfile
import threading
import unittest
from copy import deepcopy
from unittest.mock import Mock, patch

from speech_to_text.desktop import Desktop


class DesktopTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        env = patch.dict(os.environ, {'HOME': self.temp.name})
        env.start()
        self.addCleanup(env.stop)
        self.desktop = Desktop(Mock())
        self.desktop.client = Mock()
        self.desktop.client.send.return_value = True
        self.desktop.config.save()
        self.before = self.desktop.config.config_file.read_bytes()

    def test_disconnected_save_restores_previous_file(self):
        self.desktop.client.send.return_value = False
        with self.assertRaisesRegex(RuntimeError, 'not applied'):
            self.desktop.command({'cmd': 'save_config', 'config': {'transcription': {'model': 'small.en'}}})
        self.assertEqual(self.before, self.desktop.config.config_file.read_bytes())
        self.assertFalse(self.desktop.saving)

    def test_settings_wait_for_daemon_acknowledgment(self):
        self.desktop.command({'cmd': 'save_config', 'config': {'transcription': {'model': 'small.en'}}})
        self.assertTrue(self.desktop.saving)
        self.desktop.client.send.assert_called_once_with({'cmd': 'reload_config'})
        self.desktop.event({'type': 'config_reloaded'})
        self.assertFalse(self.desktop.saving)
        self.assertIsNone(self.desktop.saved_before)
        self.assertEqual(self.desktop.config.get('transcription', 'model'), 'small.en')

    def test_rejected_reload_restores_previous_file(self):
        self.desktop.command({'cmd': 'save_config', 'config': {'transcription': {'model': 'small.en'}}})
        self.desktop.event({'type': 'error', 'message': 'Busy'})
        self.assertEqual(self.before, self.desktop.config.config_file.read_bytes())

    def test_disconnect_clears_pending_save_and_restores_file(self):
        self.desktop.command({'cmd': 'save_config', 'config': {'transcription': {'model': 'small.en'}}})
        self.desktop.event({'type': 'disconnected'})
        self.assertFalse(self.desktop.saving)
        self.assertEqual(self.before, self.desktop.config.config_file.read_bytes())

    def test_busy_and_invalid_settings_never_write(self):
        self.desktop.state = {'recording': True}
        with self.assertRaises(RuntimeError):
            self.desktop.command({'cmd': 'save_config', 'config': {'audio': {'preprocess': False}}})
        self.desktop.state = {}
        with self.assertRaises(ValueError):
            self.desktop.command({'cmd': 'save_config', 'config': {'transcription': {'beam_size': 0}}})
        self.assertEqual(self.before, self.desktop.config.config_file.read_bytes())

    def test_history_search_and_config_preserve_user_data(self):
        self.desktop.history.add('First thought')
        self.desktop.history.add('Second thought')
        self.desktop.command({'cmd': 'history', 'search': 'second'})
        event = self.desktop.emit.call_args.args[0]
        self.assertEqual([i['text'] for i in event['items']], ['Second thought'])
        with patch('speech_to_text.desktop.microphones', return_value=[]):
            self.desktop.command({'cmd': 'get_config'})
        self.assertEqual(self.before, self.desktop.config.config_file.read_bytes())
        self.assertEqual(len(self.desktop.history.get_recent()), 2)

    def test_microphone_test_does_not_start_during_dictation(self):
        self.desktop.state = {'processing': True}
        with patch('speech_to_text.desktop.MicrophoneTest') as recorder:
            with self.assertRaises(RuntimeError):
                self.desktop.command({'cmd': 'test_microphone'})
            recorder.assert_not_called()

    def test_headless_import_has_no_gtk_dependency(self):
        import sys
        self.assertNotIn('speech_to_text.gui.gtk', sys.modules)

    def test_close_during_key_capture_restores_listening_before_disconnect(self):
        started = threading.Event()
        def capture():
            self.desktop.client.send({'cmd': 'set_listening', 'value': False})
            started.set()
            self.desktop.stop.wait(2)
            self.desktop.client.send({'cmd': 'set_listening', 'value': True})
        self.desktop.capture_hotkey = capture
        desktop = self.desktop
        class Input:
            def __iter__(self):
                desktop.submit({'cmd': 'capture_hotkey'})
                if not started.wait(2):
                    raise AssertionError('Capture did not start')
                return iter(())
        with patch('speech_to_text.desktop.sys.stdin', Input()):
            self.desktop.run()
        calls = self.desktop.client.method_calls
        names = [call[0] for call in calls]
        self.assertEqual(names, ['start', 'send', 'send', 'stop'])
        self.assertTrue(calls[2].args[0]['value'])
