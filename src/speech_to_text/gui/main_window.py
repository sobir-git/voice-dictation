import json
import math
import threading
import time

from speech_to_text.core.app_logging import log_path
from speech_to_text.core.diagnostics import recent_logs, report
from speech_to_text.gui.gtk import GLib, Gtk
from speech_to_text.gui.settings_dialog import SettingsPanel, friendly_key_name

import gi
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk

CSS = b'''
window.dictation { background: #171b23; color: #e2e8f3; font-family: Ubuntu, Cantarell, sans-serif; font-size: 14px; }
.dictation headerbar { background: #1c222d; color: #e2e8f3; border-bottom: 1px solid #30394a; box-shadow: none; }
.dictation headerbar .subtitle { color: #9baac2; }
.dictation .sidebar { background: #121720; border-right: 1px solid #30394a; padding: 18px 12px; }
.dictation .sidebar button { background: transparent; border: none; box-shadow: none; padding: 12px 16px; border-radius: 6px; color: #a4b2c9; }
.dictation .sidebar button:checked { background: #293d60; color: #c0d3ff; font-weight: bold; }
.dictation .brand { font-size: 19px; font-weight: bold; color: #d0def8; }
.dictation .page-title { font-size: 27px; font-weight: bold; color: #eef3ff; }
.dictation .headline { font-size: 35px; font-weight: bold; color: #eef3ff; }
.dictation .secondary { color: #a3b0c7; }
.dictation .small { font-size: 12px; }
.dictation .keycap { background: #263248; color: #d9e6ff; border: 1px solid #4a6086; border-bottom: 3px solid #4a6086; padding: 9px 18px; border-radius: 7px; font-weight: bold; }
.dictation .transcript { background: #202733; padding: 20px; border: 1px solid #344055; border-radius: 8px; }
.dictation button { background: #2a3444; background-image: none; color: #e2e8f3; border: 1px solid #44516a; padding: 8px 14px; border-radius: 6px; box-shadow: none; text-shadow: none; }
.dictation button:hover { background: #34445d; }
.dictation button:disabled { color: #75839b; background: #222b3a; border-color: #344055; }
.dictation button.suggested-action { background: #4a70c9; color: #ffffff; border-color: #668ada; }
.dictation button.suggested-action:hover { background: #5a80d9; }
.dictation button:focus { outline-color: #98b9ff; }
.dictation .error-banner { background: #422b2b; color: #ffcac0; border-bottom: 1px solid #77504d; padding: 12px 20px; }
.dictation notebook { background: #1c232f; border: 1px solid #344055; border-radius: 6px; }
.dictation notebook stack { background: #1c232f; }
.dictation notebook header { background: #222c3d; }
.dictation notebook tab { padding: 9px 15px; color: #a3b0c7; }
.dictation notebook tab:checked { color: #e2eaff; box-shadow: inset 0 -3px #89adff; border-color: #89adff; }
.dictation entry, .dictation textview text, .dictation list { background: #1c232f; color: #e2e8f3; }
.dictation entry { border: 1px solid #44516a; caret-color: #e2e8f3; }
.dictation .history-row { padding: 16px; border-bottom: 1px solid #344055; }
.dictation levelbar trough { background: #121720; border: 1px solid #344055; }
.dictation levelbar block.empty { background: #202b3e; border-color: #202b3e; }
.dictation levelbar block.filled { background: #89adff; border-color: #89adff; }
.dictation .status-dot { color: #89adff; }
menu, menuitem, popover { background: #202733; color: #e2e8f3; }
menuitem:hover { background: #34445d; }
''' 


def style(widget, name):
    widget.get_style_context().add_class(name)
    return widget


def label(text='', cls=None, wrap=False):
    result = Gtk.Label(label=text, xalign=0)
    if cls:
        style(result, cls)
    if wrap:
        result.set_line_wrap(True)
        result.set_max_width_chars(62)
    return result


def button(text, callback, primary=False):
    result = Gtk.Button(label=text)
    result.connect('clicked', callback)
    if primary:
        style(result, 'suggested-action')
    return result


class SignalDrawing(Gtk.DrawingArea):
    """A quiet recording motif; animation denotes recording, never a fake audio level."""
    def __init__(self):
        super().__init__()
        self.set_size_request(-1, 88)
        self.active = False
        self.level = 0
        self.phase = 0
        self.connect('draw', self._draw)

    def _draw(self, widget, cr):
        width, height = self.get_allocated_width(), self.get_allocated_height()
        for i in range(45):
            distance = abs(i-22)/22
            amplitude = (1-distance)**1.5
            rhythm = 0.5 + 0.5*math.sin(i*1.65 + self.phase)
            bar = 5 + amplitude*(60*self.level if self.active else 24)*rhythm
            cr.set_source_rgb(*( (1.0, 0.44, 0.42) if self.active else (0.54, 0.68, 1.0)))
            cr.rectangle(width/2 + (i-22)*9-2, height/2-bar/2, 4, bar)
            cr.fill()


class DictationWindow(Gtk.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title='Voice Dictation')
        self.app = app
        self.set_default_size(1000, 720)
        self.set_size_request(850, 620)
        style(self, 'dictation')
        provider = Gtk.CssProvider()
        provider.load_from_data(CSS)
        Gtk.StyleContext.add_provider_for_screen(self.get_screen(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        header = Gtk.HeaderBar(title='Voice Dictation', subtitle='Local speech, ready to type')
        header.set_show_close_button(True)
        self.set_titlebar(header)
        self.connection = label('Connecting', 'secondary')
        header.pack_end(self.connection)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.add(root)
        self.error_box = style(Gtk.Box(spacing=12), 'error-banner')
        self.error_text = label('', wrap=True)
        self.error_text.set_hexpand(True)
        self.error_box.pack_start(self.error_text, True, True, 0)
        self.error_box.pack_end(button('Dismiss', self._dismiss), False, False, 0)
        self.error_box.set_no_show_all(True)
        root.pack_start(self.error_box, False, False, 0)
        body = Gtk.Box()
        root.pack_start(body, True, True, 0)
        sidebar = style(Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8), 'sidebar')
        sidebar.set_size_request(180, -1)
        brand = label('Voice\nDictation', 'brand')
        brand.set_margin_start(16)
        brand.set_margin_bottom(22)
        sidebar.pack_start(brand, False, False, 0)
        body.pack_start(sidebar, False, False, 0)
        self.stack = Gtk.Stack()
        self.stack.set_hexpand(True)
        self.stack.set_vexpand(True)
        body.pack_start(self.stack, True, True, 0)
        self.nav = {}
        self._navigating = False
        self.settings = None
        for name, title, builder in (('dictation', 'Dictation', self._dictation_page),
                                      ('history', 'History', self._history_page),
                                      ('settings', 'Settings', self._settings_page),
                                      ('diagnostics', 'Diagnostics', self._diagnostics_page)):
            nav = Gtk.ToggleButton(label=title)
            nav.connect('clicked', lambda _button, page=name: self.show_page(page))
            sidebar.pack_start(nav, False, False, 0)
            self.nav[name] = nav
            self.stack.add_named(builder(), name)
        footer = label('On this computer\nPowered by Whisper', 'secondary')
        footer.set_margin_start(16)
        footer.set_margin_bottom(10)
        sidebar.pack_end(footer, False, False, 0)
        self.connect('delete-event', self._hide)
        self.connect('destroy', self._destroy)
        self._timer = None
        self._recording_since = None
        self._diagnostics_busy = False
        self._log_timer = None
        self._history_refresh = None
        self.show_all()
        self.show_page('dictation')
        self.update_state()

    def _page(self, title, subtitle):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=18)
        for method in (box.set_margin_start, box.set_margin_end, box.set_margin_top, box.set_margin_bottom):
            method(28)
        box.pack_start(label(title, 'page-title'), False, False, 0)
        box.pack_start(label(subtitle, 'secondary', wrap=True), False, False, 0)
        return box

    def _dictation_page(self):
        box = self._page('Your voice, in any app', 'Place the cursor where you want your words. Hold your hotkey and speak.')
        stage = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=14)
        stage.set_vexpand(True)
        stage.set_valign(Gtk.Align.CENTER)
        self.signal = SignalDrawing()
        stage.pack_start(self.signal, False, False, 0)
        self.headline = label('Connecting…', 'headline')
        self.headline.set_halign(Gtk.Align.CENTER)
        stage.pack_start(self.headline, False, False, 0)
        self.instruction = label('Starting the dictation service', 'secondary')
        self.instruction.set_halign(Gtk.Align.CENTER)
        stage.pack_start(self.instruction, False, False, 0)
        self.hotkey = style(label('Right Ctrl'), 'keycap')
        self.hotkey.set_halign(Gtk.Align.CENTER)
        self.hotkey.set_margin_top(4)
        stage.pack_start(self.hotkey, False, False, 0)
        box.pack_start(stage, True, True, 0)
        actions = Gtk.Box(spacing=12)
        self.pause = button('Pause listening', self._toggle)
        actions.pack_start(self.pause, False, False, 0)
        self.cancel_button = button('Cancel dictation', lambda *_: self.app.client.send({'cmd': 'cancel'}))
        actions.pack_start(self.cancel_button, False, False, 0)
        actions.pack_start(button('Microphone settings', lambda *_: self.show_page('settings')), False, False, 0)
        box.pack_start(actions, False, False, 0)
        self.mic_label = label('', 'secondary', wrap=True)
        self.mic_label.set_ellipsize(3)
        box.pack_start(self.mic_label, False, False, 0)
        transcript = style(Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12), 'transcript')
        row = Gtk.Box()
        row.pack_start(label('Latest dictation', 'secondary'), True, True, 0)
        row.pack_end(button('Copy', self._copy_latest), False, False, 0)
        transcript.pack_start(row, False, False, 0)
        self.latest = label('Your next dictation will appear here.', wrap=True)
        self.latest.set_selectable(True)
        transcript.pack_start(self.latest, False, False, 0)
        self.timing = label('Saved locally in History', 'secondary')
        transcript.pack_start(self.timing, False, False, 0)
        box.pack_start(transcript, False, False, 0)
        self._latest_text = ''
        recent = self.app.history.get_recent(1)
        if recent:
            self._latest_text = recent[0]['text']
            self.latest.set_text(self._preview(self._latest_text))
        return box

    @staticmethod
    def _preview(text):
        return text[:260] + ('…' if len(text) > 260 else '')

    def _history_page(self):
        box = self._page('Dictation history', 'Your words stay here, including when typing into an app fails.')
        self.search = Gtk.SearchEntry(placeholder_text='Search your dictations')
        self.search.connect('search-changed', self._search_changed)
        box.pack_start(self.search, False, False, 0)
        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self.history_list = Gtk.ListBox()
        self.history_list.set_selection_mode(Gtk.SelectionMode.NONE)
        scroll.add(self.history_list)
        box.pack_start(scroll, True, True, 0)
        return box

    def _search_changed(self, *_):
        if getattr(self, '_history_refresh', None):
            GLib.source_remove(self._history_refresh)
        self._history_refresh = GLib.timeout_add(150, self._refresh_history)

    def _refresh_history(self):
        self._history_refresh = None
        for child in self.history_list.get_children():
            child.destroy()
        items = self.app.history.get_recent(100, self.search.get_text())
        if not items:
            empty = label('No matching dictations.' if self.search.get_text() else 'No dictations yet. Hold your hotkey to record your first one.', 'secondary', True)
            empty.set_margin_top(30)
            self.history_list.add(empty)
        for item in items:
            row = style(Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10), 'history-row')
            meta = Gtk.Box()
            meta.pack_start(label(item['timestamp'], 'secondary'), True, True, 0)
            meta.pack_end(button('Copy', lambda _, text=item['text']: self._copy(text)), False, False, 0)
            row.pack_start(meta, False, False, 0)
            words = label(item['text'], wrap=True)
            words.set_selectable(True)
            row.pack_start(words, False, False, 0)
            self.history_list.add(row)
        self.history_list.show_all()
        return False

    def _settings_page(self):
        box = self._page('Make it yours', 'Choose your microphone, hotkey, and transcription preferences.')
        self.settings = SettingsPanel(self, self.app.config, self.app.client)
        box.pack_start(self.settings, True, True, 0)
        row = Gtk.Box(spacing=12)
        self.save_button = button('Save changes', self._save_settings, True)
        row.pack_start(self.save_button, False, False, 0)
        self.save_status = label('', 'secondary', True)
        row.pack_start(self.save_status, True, True, 0)
        box.pack_end(row, False, False, 0)
        return box

    def _save_settings(self, *_):
        if self.app.is_recording or self.app.is_processing:
            self.save_status.set_text('Finish the current dictation before saving.')
            return
        try:
            self.settings.save()
            self.save_status.set_text('Saved. Waiting for the service to apply changes…')
        except Exception as exc:
            self.save_status.set_text(str(exc))

    def _diagnostics_page(self):
        box = self._page('See what is happening', 'Connection, microphone routing, dependencies, and recent logs in one place.')
        row = Gtk.Box(spacing=10)
        self.debug = Gtk.CheckButton(label='Detailed debug logging')
        self.debug.set_active(self.app.config.get('logging', 'level') == 'DEBUG')
        self.debug.connect('toggled', self._debug_changed)
        row.pack_start(self.debug, True, True, 0)
        self.follow_logs = Gtk.CheckButton(label='Follow logs')
        self.follow_logs.connect('toggled', self._follow_logs_changed)
        row.pack_start(self.follow_logs, False, False, 0)
        row.pack_end(button('Refresh', self._refresh_diagnostics), False, False, 0)
        row.pack_end(button('Copy report', lambda *_: self._copy(self._diagnostic_text())), False, False, 0)
        box.pack_start(row, False, False, 0)
        scroll = Gtk.ScrolledWindow()
        self.log_view = Gtk.TextView(editable=False, cursor_visible=False)
        self.log_view.set_monospace(True)
        self.log_view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self.log_view.set_left_margin(14)
        self.log_view.set_right_margin(14)
        self.log_view.set_top_margin(14)
        scroll.add(self.log_view)
        box.pack_start(scroll, True, True, 0)
        box.pack_start(label('Debug logs record timings and errors, not your spoken text. Reports include local device names and paths.', 'secondary', True), False, False, 0)
        return box

    def _follow_logs_changed(self, *_):
        if self._log_timer is not None:
            GLib.source_remove(self._log_timer)
            self._log_timer = None
        if self.follow_logs.get_active():
            self._refresh_diagnostics()
            self._log_timer = GLib.timeout_add_seconds(2, self._follow_tick)

    def _follow_tick(self):
        if self.get_visible() and self.stack.get_visible_child_name() == 'diagnostics':
            self._refresh_diagnostics()
        return True

    def _diagnostic_text(self):
        buffer = self.log_view.get_buffer()
        return buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), False)

    def _refresh_diagnostics(self, *_):
        if self._diagnostics_busy:
            return
        self._diagnostics_busy = True
        def collect():
            try:
                text = json.dumps(report(self.app.config), indent=2) + '\n\nRecent logs\n\n' + recent_logs(self.app.config)
            except Exception as exc:
                text = f'Diagnostics failed: {exc}'
            GLib.idle_add(self._set_diagnostics, text)
        threading.Thread(target=collect, name='diagnostics', daemon=True).start()

    def _set_diagnostics(self, text):
        self._diagnostics_busy = False
        if self.get_realized():
            buffer = self.log_view.get_buffer()
            buffer.set_text(text)
            if self.follow_logs.get_active():
                self.log_view.scroll_to_iter(buffer.get_end_iter(), 0, False, 0, 0)
        return False

    def _debug_changed(self, *_):
        try:
            level = 'DEBUG' if self.debug.get_active() else 'INFO'
            if not self.app.client.send({'cmd': 'set_log_level', 'value': level}):
                raise RuntimeError('Reconnect to the service before changing the log level.')
        except Exception as exc:
            self.show_error(str(exc))

    def show_page(self, page):
        if self._navigating:
            return
        self._navigating = True
        if self.settings and page != 'settings' and self.settings._test:
            self.settings.stop_test()
        self.stack.set_visible_child_name(page)
        for name, nav in self.nav.items():
            nav.set_active(name == page)
        self._navigating = False
        if page == 'history':
            self._refresh_history()
        elif page == 'diagnostics':
            self._refresh_diagnostics()

    def update_state(self):
        app = self.app
        model = app.config.get('transcription', 'model')
        self.connection.set_text(f'{model} ready' if app.is_connected and app.model_ready else 'Connecting / loading…')
        self.pause.set_sensitive(app.is_connected)
        self.cancel_button.set_sensitive(app.is_connected and (app.is_recording or app.is_processing))
        self.pause.set_label('Pause listening' if app.is_listening else 'Enable listening')
        self.save_button.set_sensitive(app.is_connected and not app.is_recording and not app.is_processing)
        self.hotkey.set_text(friendly_key_name(app.config.get('input', 'trigger_key')))
        node = app.config.get('audio', 'pipewire_node')
        self.mic_label.set_text('Microphone: ' + (app.microphone_name or node or 'System default'))
        if app.is_recording and self._timer is None:
            self._timer = GLib.timeout_add(100, self._tick)
        elif not app.is_recording and self._timer is not None:
            GLib.source_remove(self._timer)
            self._timer = None
        if app.is_recording and not app.capture_ready:
            self.signal.level = 0
        self.signal.active = app.is_recording
        self.signal.queue_draw()
        if not app.is_connected:
            title, hint = 'Connecting…', 'Starting the dictation service'
        elif app.is_recording:
            if app.capture_ready:
                title, hint = 'Listening to you', 'Release your hotkey to transcribe'
            else:
                title, hint = 'Opening microphone', 'Waiting for the first audio samples'
        elif app.cancel_requested and app.is_processing:
            title, hint = 'Canceling dictation', 'Finishing active processing without typing'
        elif app.is_processing:
            title, hint = 'Finding your words', 'Your text will appear in the active app'
        elif not app.is_listening:
            title, hint = 'Take a pause', 'Enable listening when you are ready'
        elif not app.model_ready:
            title, hint = 'Loading your model', 'The first download can take a few minutes'
        else:
            title, hint = 'Ready when you are', 'Hold your hotkey. Speak. Release.'
        self.headline.set_text(title)
        self.instruction.set_text(hint)
        if app.is_recording and app.capture_ready and self._recording_since is None:
            self._recording_since = time.monotonic()
        elif not app.is_recording:
            self._recording_since = None

    def transcription(self, text, duration):
        self._latest_text = text
        self.latest.set_text(self._preview(text))
        self.timing.set_text(f'Transcribed in {duration:.2f} seconds · Saved in History')
        if self.stack.get_visible_child_name() == 'history':
            self._refresh_history()

    def show_error(self, text):
        self.error_text.set_text(text)
        self.error_box.show_all()
        self.error_box.show()

    def _dismiss(self, *_):
        self.error_box.hide()
        self.app.last_error = ''
        self.app.client.send({'cmd': 'clear_error'})

    def _toggle(self, *_):
        self.app.client.send({'cmd': 'set_listening', 'value': not self.app.is_listening})

    def _copy(self, text):
        clipboard = Gtk.Clipboard.get_default(self.get_display())
        clipboard.set_text(text, -1)
        clipboard.store()

    def _copy_latest(self, *_):
        self._copy(self._latest_text)

    def _tick(self):
        if self.get_visible() and self.app.is_recording and self.app.capture_ready:
            self.signal.phase += 0.3
            self.signal.queue_draw()
            if self._recording_since:
                elapsed = int(time.monotonic()-self._recording_since)
                self.instruction.set_text(f'{elapsed//60}:{elapsed%60:02d} recording · Release your hotkey to transcribe')
        return True

    def _hide(self, *_):
        self.settings.stop_test()
        self.hide()
        return True

    def _destroy(self, *_):
        if self._timer is not None:
            GLib.source_remove(self._timer)
        if self._history_refresh:
            GLib.source_remove(self._history_refresh)
        if self._log_timer is not None:
            GLib.source_remove(self._log_timer)
