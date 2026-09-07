import json
import os
import socket
import tempfile
import threading
import time
import unittest
import wave
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from speech_to_text.core.config import Config
from speech_to_text.core.controller import SpeechToTextController
from speech_to_text.core.audio_recorder import AudioRecorder
from speech_to_text.core.hotkey_listener import HotkeyListener
from speech_to_text.core.transcriber import Transcriber
from speech_to_text.core.text_output import TextOutput
from speech_to_text.daemon import IPCServer
from speech_to_text.gui.ipc_client import DaemonClient
from evdev import ecodes


def wait_for(predicate, timeout=3):
    deadline = time.monotonic()+timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.01)
    raise AssertionError('Condition did not become true')


class IsolatedConfig(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.env = patch.dict(os.environ, {'HOME': self.temp.name})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.config = Config()
        self.config.data['audio'].update(temp_file=self.temp.name+'/recording.wav', preprocess=False)

    def test_invalid_reload_preserves_last_good_config(self):
        self.config.save()
        old = self.config.data.copy()
        self.config.config_file.write_text('audio: null')
        with self.assertRaises(ValueError):
            self.config.load()
        self.assertEqual(self.config.data, old)

    def test_atomic_save_does_not_truncate_on_failure(self):
        self.config.save()
        before = self.config.config_file.read_text()
        with patch('speech_to_text.core.config.os.replace', side_effect=OSError('disk failure')):
            with self.assertRaises(OSError):
                self.config.save()
        self.assertEqual(self.config.config_file.read_text(), before)
        self.assertEqual(list(self.config.config_dir.glob('.config-*')), [])

    def test_boolean_as_integer_rejected(self):
        self.config.data['transcription']['beam_size'] = True
        with self.assertRaises(ValueError):
            self.config.save()

    def controller(self):
        for name in ('Transcriber', 'TextOutput', 'Notifier'):
            patcher = patch('speech_to_text.core.controller.'+name)
            patcher.start()
            self.addCleanup(patcher.stop)
        controller = SpeechToTextController(self.config)
        controller.is_listening = True
        self.addCleanup(controller.stop)
        controller.recorder = FakeRecorder(self.temp.name)
        return controller

    def test_overlapping_dictations_use_distinct_files_and_preserve_order(self):
        c = self.controller()
        entered, release = threading.Event(), threading.Event()
        def transcribe(path):
            if path.endswith('1.wav'):
                entered.set()
                release.wait(2)
            return Path(path).read_text()
        c.transcriber.transcribe_file.side_effect = transcribe
        c.start_recording()
        c.stop_recording()
        self.assertTrue(entered.wait(1))
        c.start_recording()
        c.stop_recording()
        self.assertEqual(c._pending, 2)
        self.assertTrue(Path(self.temp.name+'/1.wav').exists())
        release.set()
        wait_for(lambda: c._pending == 0)
        self.assertEqual([call.args[0] for call in c.text_output.type_text.call_args_list], ['recording 1', 'recording 2'])
        self.assertFalse(c.is_processing)
        self.assertFalse(list(Path(self.temp.name).glob('*.wav')))

    def test_failed_capture_does_not_leave_recording_state(self):
        c = self.controller()
        c.recorder.start = Mock(side_effect=OSError('microphone unavailable'))
        c.start_recording()
        self.assertFalse(c.is_recording)
        self.assertEqual(c._pending, 0)

    def test_queue_is_bounded(self):
        c = self.controller()
        c._pending = c.MAX_PENDING
        c.recorder.start = Mock()
        c.start_recording()
        c.recorder.start.assert_not_called()
        c._pending = 0

    def test_history_received_before_failed_output(self):
        c = self.controller()
        c.transcriber.transcribe_file.return_value = 'recover these words'
        c.text_output.type_text.side_effect = RuntimeError('output unavailable')
        c.on_transcription = Mock()
        c.start_recording()
        c.stop_recording()
        wait_for(lambda: c._pending == 0)
        c.on_transcription.assert_called_once()
        self.assertEqual(c.on_transcription.call_args.args[0], 'recover these words')

    def test_shutdown_never_starts_a_new_transcription(self):
        c = self.controller()
        c.start_recording()
        c.stop()
        c.transcriber.transcribe_file.assert_not_called()
        self.assertFalse(list(Path(self.temp.name).glob('*.wav')))

    def test_reload_rejected_while_busy(self):
        c = self.controller()
        c.start_recording()
        with self.assertRaisesRegex(RuntimeError, 'finish'):
            c.reload_config()

    def test_reload_preserves_paused_state_and_loaded_model(self):
        c = self.controller()
        c.is_listening = False
        c.config.save()
        original = c.transcriber
        with patch.object(c, '_start_listener'):
            c.reload_config()
        self.assertIs(c.transcriber, original)
        self.assertFalse(c.is_listening)


class FakeRecorder:
    def __init__(self, directory):
        self.directory, self.count = directory, 0
    def start(self):
        self.count += 1
        self.output_file = f'{self.directory}/{self.count}.wav'
        Path(self.output_file).write_text(f'recording {self.count}')
    def stop(self):
        return True
    def discard(self):
        Path(self.output_file).unlink(missing_ok=True)


class RecorderTests(unittest.TestCase):
    def test_silent_microphone_is_reported_before_transcription(self):
        with tempfile.TemporaryDirectory() as directory:
            path = directory+'/silent.wav'
            with wave.open(path, 'wb') as audio:
                audio.setparams((1, 2, 16000, 0, 'NONE', ''))
                audio.writeframes(b'\0'*32000)
            recorder = AudioRecorder(output_file=path)
            recorder.process = Mock()
            recorder.process.poll.return_value = None
            with self.assertRaisesRegex(RuntimeError, 'silence'):
                recorder.stop()
            self.assertIsNone(recorder.process)

    def test_missing_arecord_cleans_up_private_file(self):
        with tempfile.TemporaryDirectory() as directory:
            recorder = AudioRecorder(output_file=directory+'/recording.wav')
            with patch('speech_to_text.core.audio_recorder.subprocess.Popen', side_effect=FileNotFoundError()):
                with self.assertRaises(FileNotFoundError):
                    recorder.start()
            self.assertEqual(list(Path(directory).iterdir()), [])
            self.assertIsNone(recorder.process)


class ListenerTests(unittest.TestCase):
    def listener(self):
        return HotkeyListener('KEY_RIGHTCTRL', Mock(), Mock(), Mock(), lambda: True, lambda: True)

    def test_device_disappearing_releases_hotkey(self):
        listener = self.listener()
        device = Mock(path='/test')
        listener._pressed_sources.add('/test')
        listener._remove(Mock(), {'/test': device}, '/test')
        listener.on_key_up.assert_called_once()
        self.assertFalse(listener._pressed_sources)

    def test_release_while_paused_is_not_lost(self):
        listener = self.listener()
        listener._pressed_sources.add('/test')
        listener.is_enabled = lambda: False
        listener._event(SimpleNamespace(path='/test'), SimpleNamespace(type=ecodes.EV_KEY, code=ecodes.KEY_RIGHTCTRL, value=0))
        listener.on_key_up.assert_called_once()

    def test_dropped_events_release_recording(self):
        listener = self.listener()
        listener._pressed_sources.add('/test')
        listener._event(SimpleNamespace(path='/test'), SimpleNamespace(type=ecodes.EV_SYN, code=ecodes.SYN_DROPPED, value=0))
        listener.on_key_up.assert_called_once()

    def test_stop_and_restart_join_previous_thread(self):
        listener = self.listener()
        with patch.object(listener, '_open_devices'):
            listener.start()
            old_thread = listener._thread
            listener.stop()
            self.assertFalse(old_thread.is_alive())
            listener.start()
            self.assertIsNot(listener._thread, old_thread)
            listener.stop()


class ModelTests(unittest.TestCase):
    def test_concurrent_preload_and_dictation_only_load_once(self):
        t = Transcriber()
        with patch('speech_to_text.core.transcriber.WhisperModel') as model:
            threads = [threading.Thread(target=t.load_model) for _ in range(8)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(1)
            model.assert_called_once()
            self.assertTrue(model.call_args.kwargs['local_files_only'])


class OutputTests(unittest.TestCase):
    def test_failed_output_is_not_reported_as_success(self):
        import subprocess
        output = TextOutput('xdotool')
        with patch('speech_to_text.core.text_output.subprocess.run', side_effect=subprocess.CalledProcessError(1, ['secret words'], stderr=b'disconnected')):
            with self.assertRaisesRegex(RuntimeError, 'History') as caught:
                output.type_text('secret words')
        self.assertNotIn('secret words', str(caught.exception))

    def test_no_auto_output_is_actionable_error(self):
        with patch.object(TextOutput, '_resolve_method', return_value='none'):
            output = TextOutput('auto')
            with self.assertRaisesRegex(RuntimeError, 'History'):
                output.type_text('some text')

    def test_explicit_none_is_history_only(self):
        TextOutput('none').type_text('some text')


class IPCTests(unittest.TestCase):
    def controller(self):
        return SimpleNamespace(is_listening=True, is_recording=False, is_processing=False,
            _pending=0, capture_ready=False, transcriber=SimpleNamespace(model=object()),
            text_output=SimpleNamespace(_resolved_method='none'), trigger_key='KEY_F16',
            config=SimpleNamespace(get=lambda *_: ''), recorder=SimpleNamespace(device='default'))

    def test_duplicate_server_cannot_unlink_running_socket(self):
        with tempfile.TemporaryDirectory() as directory:
            path = directory+'/daemon.sock'
            server = IPCServer(path, self.controller())
            server.start()
            try:
                duplicate = IPCServer(path, self.controller())
                with self.assertRaises(RuntimeError):
                    duplicate.start()
                with socket.socket(socket.AF_UNIX) as conn:
                    conn.settimeout(1)
                    conn.connect(path)
                    self.assertEqual(json.loads(conn.recv(4096))['type'], 'state')
            finally:
                server.stop()

    def test_bad_command_does_not_break_subsequent_requests(self):
        server = IPCServer('/unused', self.controller())
        server.broadcast = Mock()
        for payload in (b'[]', b'{bad', b'{"cmd":"set_listening","value":"false"}'):
            server._dispatch(payload)
        server._dispatch(b'{"cmd":"get_state"}')
        self.assertEqual(server.broadcast.call_args.args[0]['type'], 'state')

    def test_failed_client_send_does_not_deadlock(self):
        client = DaemonClient('/unused', Mock())
        client._sock = Mock()
        client._sock.sendall.side_effect = BrokenPipeError()
        thread = threading.Thread(target=lambda: client.send({'cmd': 'get_state'}), daemon=True)
        thread.start()
        thread.join(1)
        self.assertFalse(thread.is_alive())
        self.assertIsNone(client._sock)

    def test_stop_interrupts_client_read(self):
        left, right = socket.socketpair()
        try:
            client = DaemonClient('/unused', Mock())
            client._sock = left
            received = []
            thread = threading.Thread(target=lambda: received.append(left.recv(1)), daemon=True)
            thread.start()
            client.stop()
            thread.join(1)
            self.assertFalse(thread.is_alive())
        finally:
            right.close()




class AdditionalConcurrencyTests(unittest.TestCase):
    setUp = IsolatedConfig.setUp
    controller = IsolatedConfig.controller
    def test_output_waits_for_the_next_hotkey_release(self):
        c = self.controller()
        entered, release = threading.Event(), threading.Event()
        def transcribe(path):
            entered.set()
            release.wait(2)
            return 'first dictation'
        c.transcriber.transcribe_file.side_effect = transcribe
        c.start_recording()
        c.stop_recording()
        self.assertTrue(entered.wait(1))
        c.start_recording()
        release.set()
        time.sleep(.05)
        c.text_output.type_text.assert_not_called()
        c.stop_recording()
        wait_for(lambda: c._pending == 0)
        self.assertEqual(c.text_output.type_text.call_count, 2)

    def test_output_failure_does_not_break_following_dictations(self):
        c = self.controller()
        c.transcriber.transcribe_file.return_value = 'words'
        c.text_output.type_text.side_effect = [RuntimeError('failed'), None]
        c.start_recording()
        c.stop_recording()
        wait_for(lambda: c._pending == 0)
        self.assertFalse(c._outputting)
        c.start_recording()
        c.stop_recording()
        wait_for(lambda: c._pending == 0)
        self.assertEqual(c.text_output.type_text.call_count, 2)

    def test_cancel_discards_recording_without_transcribing(self):
        c = self.controller()
        c.start_recording()
        c.cancel()
        self.assertFalse(c.is_recording)
        self.assertEqual(c._pending, 0)
        c.transcriber.transcribe_file.assert_not_called()
        self.assertFalse(list(Path(self.temp.name).glob('*.wav')))

    def test_cancel_during_inference_suppresses_late_output(self):
        c = self.controller()
        entered, release = threading.Event(), threading.Event()
        def transcribe(path):
            entered.set()
            release.wait(2)
            return 'do not type this'
        c.transcriber.transcribe_file.side_effect = transcribe
        c.on_transcription = Mock()
        c.start_recording()
        c.stop_recording()
        self.assertTrue(entered.wait(1))
        c.cancel()
        release.set()
        wait_for(lambda: c._pending == 0)
        c.text_output.type_text.assert_not_called()
        c.on_transcription.assert_not_called()

if __name__ == '__main__':
    unittest.main()
