"""Run with: PYTHONPATH=src xvfb-run -a ./venv/bin/python3 tests/gui_smoke.py

Creates an isolated config/history and optionally exports screenshots with --screenshots DIR.
No microphone recording, model downloads, or typing into the user's desktop.
"""
import argparse
import os
from pathlib import Path
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--screenshots')
args = parser.parse_args()
config_dir = tempfile.TemporaryDirectory(prefix='stt-ui-test-')
os.environ['HOME'] = config_dir.name

from speech_to_text.gui.gtk import Gtk, GLib
from speech_to_text.core.config import Config
from speech_to_text.core.history import HistoryManager
from speech_to_text.gui.main_window import DictationWindow
from gi.repository import Gdk

app = Gtk.Application(application_id='com.test.voice-dictation.ui')
app.register(None)
app.config = Config()
app.config.data['input']['trigger_key'] = 'KEY_RIGHTCTRL'
app.history = HistoryManager()
app.history.add('Remember to send the updated design notes after the team meeting.')
app.client = type('Client', (), {'send': lambda *_: True})()
app.is_connected = app.is_listening = app.model_ready = True
app.is_recording = app.is_processing = False
app.microphone_name = 'Built-in digital microphone'
app.last_error = ''
app.capture_ready = True
app.cancel_requested = False
window = DictationWindow(app)
window.show()
pages = iter(('dictation', 'settings', 'history', 'diagnostics'))
errors = []

def capture():
    if args.screenshots:
        directory = Path(args.screenshots)
        directory.mkdir(parents=True, exist_ok=True)
        pixbuf = Gdk.pixbuf_get_from_window(window.get_window(), 0, 0,
                                          window.get_allocated_width(), window.get_allocated_height())
        pixbuf.savev(str(directory / (window.stack.get_visible_child_name()+'.png')), 'png', [], [])
    return False

def step():
    try:
        page = next(pages)
        window.show_page(page)
        assert window.stack.get_visible_child_name() == page
        assert sum(nav.get_active() for nav in window.nav.values()) == 1
        assert Gtk.Settings.get_default().get_property('gtk-application-prefer-dark-theme')
        GLib.timeout_add(400, capture)
        return True
    except StopIteration:
        try:
            window.show_error('Choose a working microphone in Settings.')
            assert window.error_box.get_visible()
            window._dismiss()
            assert not window.error_box.get_visible()
            app.is_recording = True
            window.update_state()
            assert window.headline.get_text() == 'Listening to you'
            app.is_recording = False
            app.is_processing = True
            window.update_state()
            assert not window.save_button.get_sensitive()
            app.is_processing = False
            window.update_state()
            window.search.set_text('nonexistent words')
            window._refresh_history()
            assert len(window.history_list.get_children()) == 1
        except Exception as exc:
            errors.append(exc)
        window.destroy()
        Gtk.main_quit()
        return False
    except Exception as exc:
        errors.append(exc)
        Gtk.main_quit()
        return False

GLib.timeout_add(750, step)
Gtk.main()
config_dir.cleanup()
if errors:
    raise errors[0]
print('Dark UI smoke checks passed: pages, navigation, error dismissal, recording states, and history search.')
