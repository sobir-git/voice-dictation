#!/usr/bin/env python3
"""
Speech-to-Text System Tray Application (Ubuntu Native)
Uses AppIndicator3/AyatanaAppIndicator3 + GTK3 for proper Ubuntu tray support.
"""

import os
import sys
import signal
import logging
import threading
import fcntl
from pathlib import Path

import gi

gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib

try:
    gi.require_version('AppIndicator3', '0.1')
    from gi.repository import AppIndicator3 as AppIndicator
except ValueError:
    gi.require_version('AyatanaAppIndicator3', '0.1')
    from gi.repository import AyatanaAppIndicator3 as AppIndicator

sys.path.insert(0, str(Path(__file__).parent))

from stt_core.config import Config
from stt_core.app_logging import setup_logging
from stt_core.controller import SpeechToTextController
from stt_core.cursor_indicator import CursorIndicator
from stt_gui.history_manager import HistoryManager

LOCK_FILE = '/tmp/stt_tray.lock'
ICON_LISTENING = 'audio-input-microphone'
ICON_PAUSED = 'microphone-sensitivity-muted'
ICON_RECORDING = 'media-record'
ICON_PROCESSING = 'emblem-synchronizing'


def acquire_lock():
    lock_fd = open(LOCK_FILE, 'w')
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        lock_fd.write(str(os.getpid()))
        lock_fd.flush()
        return lock_fd
    except IOError:
        print('Another instance is already running. Exiting.')
        sys.exit(1)


class SettingsDialog(Gtk.Dialog):
    def __init__(self, parent, config: Config):
        super().__init__(title='Speech-to-Text Settings', transient_for=parent, flags=0)
        self.config = config
        self.set_default_size(500, 400)
        self.add_buttons(Gtk.STOCK_CANCEL, Gtk.ResponseType.CANCEL, Gtk.STOCK_SAVE, Gtk.ResponseType.OK)

        notebook = Gtk.Notebook()
        self.get_content_area().pack_start(notebook, True, True, 0)

        notebook.append_page(self._build_transcription_tab(), Gtk.Label(label='Transcription'))
        notebook.append_page(self._build_input_tab(), Gtk.Label(label='Input'))
        notebook.append_page(self._build_output_tab(), Gtk.Label(label='Output'))

        self.show_all()

    def _build_transcription_tab(self) -> Gtk.Widget:
        grid = Gtk.Grid(column_spacing=12, row_spacing=8)
        grid.set_margin_top(12)
        grid.set_margin_bottom(12)
        grid.set_margin_start(12)
        grid.set_margin_end(12)

        row = 0

        grid.attach(Gtk.Label(label='Model:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.model_combo = Gtk.ComboBoxText()
        models = ('tiny.en', 'base.en', 'small.en', 'medium.en', 'large-v3')
        for m in models:
            self.model_combo.append(m, m)
        current_model = str(self.config.get('transcription', 'model', default='base.en'))
        if not self.model_combo.set_active_id(current_model):
            self.model_combo.set_active(1)
        grid.attach(self.model_combo, 1, row, 1, 1)
        row += 1

        grid.attach(Gtk.Label(label='Compute type:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.compute_combo = Gtk.ComboBoxText()
        compute_types = ('int8', 'int8_float16', 'float16', 'float32')
        for c in compute_types:
            self.compute_combo.append(c, c)
        current_compute = str(self.config.get('transcription', 'compute_type', default='int8'))
        if not self.compute_combo.set_active_id(current_compute):
            self.compute_combo.set_active(0)
        grid.attach(self.compute_combo, 1, row, 1, 1)
        row += 1

        grid.attach(Gtk.Label(label='Language:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.lang_entry = Gtk.Entry()
        self.lang_entry.set_text(str(self.config.get('transcription', 'language', default='en')))
        grid.attach(self.lang_entry, 1, row, 1, 1)
        row += 1

        grid.attach(Gtk.Label(label='Accuracy (beam size):', halign=Gtk.Align.START), 0, row, 1, 1)
        self.beam_spin = Gtk.SpinButton.new_with_range(1, 10, 1)
        self.beam_spin.set_value(int(self.config.get('transcription', 'beam_size', default=1) or 1))
        grid.attach(self.beam_spin, 1, row, 1, 1)
        row += 1

        self.vad_check = Gtk.CheckButton(label='Remove silence (VAD)')
        self.vad_check.set_active(bool(self.config.get('transcription', 'vad_filter', default=True)))
        grid.attach(self.vad_check, 1, row, 1, 1)

        return grid

    def _build_input_tab(self) -> Gtk.Widget:
        grid = Gtk.Grid(column_spacing=12, row_spacing=8)
        grid.set_margin_top(12)
        grid.set_margin_bottom(12)
        grid.set_margin_start(12)
        grid.set_margin_end(12)

        row = 0

        grid.attach(Gtk.Label(label='Hotkey:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.key_combo = Gtk.ComboBoxText()
        keys = ('KEY_F13', 'KEY_F14', 'KEY_F15', 'KEY_F16', 'KEY_F17', 'KEY_F18')
        for k in keys:
            self.key_combo.append(k, k)
        current_key = str(self.config.get('input', 'trigger_key', default='KEY_F16'))
        if not self.key_combo.set_active_id(current_key):
            self.key_combo.set_active(3)
        grid.attach(self.key_combo, 1, row, 1, 1)
        row += 1

        grid.attach(Gtk.Label(label='Device path (empty=auto):', halign=Gtk.Align.START), 0, row, 1, 1)
        self.device_entry = Gtk.Entry()
        self.device_entry.set_text(str(self.config.get('input', 'device_path', default='') or ''))
        grid.attach(self.device_entry, 1, row, 1, 1)

        return grid

    def _build_output_tab(self) -> Gtk.Widget:
        grid = Gtk.Grid(column_spacing=12, row_spacing=8)
        grid.set_margin_top(12)
        grid.set_margin_bottom(12)
        grid.set_margin_start(12)
        grid.set_margin_end(12)

        row = 0

        grid.attach(Gtk.Label(label='Output method:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.out_combo = Gtk.ComboBoxText()
        methods = ('auto', 'xdotool', 'ydotool', 'dotool', 'wtype', 'xclip', 'none')
        for o in methods:
            self.out_combo.append(o, o)
        current_method = str(self.config.get('output', 'method', default='auto'))
        if not self.out_combo.set_active_id(current_method):
            self.out_combo.set_active(0)
        grid.attach(self.out_combo, 1, row, 1, 1)
        row += 1

        self.space_check = Gtk.CheckButton(label='Add space after text')
        self.space_check.set_active(bool(self.config.get('output', 'add_space', default=True)))
        grid.attach(self.space_check, 1, row, 1, 1)

        row += 1
        self.cursor_check = Gtk.CheckButton(label='Show cursor indicator (recording/processing)')
        self.cursor_check.set_active(bool(self.config.get('ui', 'cursor_indicator', default=True)))
        grid.attach(self.cursor_check, 1, row, 1, 1)

        return grid

    def save(self) -> None:
        self.config.data.setdefault('transcription', {})
        self.config.data.setdefault('input', {})
        self.config.data.setdefault('output', {})
        self.config.data.setdefault('ui', {})

        self.config.data['transcription'].update({
            'model': self.model_combo.get_active_text() or 'base.en',
            'compute_type': self.compute_combo.get_active_text() or 'int8',
            'language': self.lang_entry.get_text() or 'en',
            'beam_size': int(self.beam_spin.get_value()),
            'vad_filter': self.vad_check.get_active(),
        })
        self.config.data['input'].update({
            'trigger_key': self.key_combo.get_active_text() or 'KEY_F16',
            'device_path': self.device_entry.get_text().strip(),
        })
        self.config.data['output'].update({
            'method': self.out_combo.get_active_text() or 'auto',
            'add_space': self.space_check.get_active(),
        })

        self.config.data['ui'].update({
            'cursor_indicator': self.cursor_check.get_active(),
        })
        self.config.save()


class HistoryDialog(Gtk.Dialog):
    def __init__(self, parent, history: HistoryManager):
        super().__init__(title='Transcription History', transient_for=parent, flags=0)
        self.history = history
        self.set_default_size(600, 400)
        self.add_buttons(Gtk.STOCK_CLOSE, Gtk.ResponseType.CLOSE)

        self.store = Gtk.ListStore(int, str, str)
        self.tree = Gtk.TreeView(model=self.store)

        renderer = Gtk.CellRendererText()
        col_time = Gtk.TreeViewColumn('Time', renderer, text=1)
        col_time.set_min_width(140)
        self.tree.append_column(col_time)

        col_text = Gtk.TreeViewColumn('Transcription', renderer, text=2)
        col_text.set_expand(True)
        self.tree.append_column(col_text)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        scroll.add(self.tree)

        box = self.get_content_area()
        box.pack_start(scroll, True, True, 0)

        btn_copy = Gtk.Button(label='Copy to Clipboard')
        btn_copy.connect('clicked', self._on_copy)
        box.pack_start(btn_copy, False, False, 8)

        self._load()
        self.show_all()

    def _load(self) -> None:
        self.store.clear()
        for item in self.history.get_recent(50):
            txt = item['text'][:80] + '...' if len(item['text']) > 80 else item['text']
            self.store.append([item['id'], item['timestamp'], txt])

    def _on_copy(self, _widget) -> None:
        sel = self.tree.get_selection()
        model, treeiter = sel.get_selected()
        if treeiter is None:
            return
        item_id = model[treeiter][0]
        item = self.history.get_by_id(item_id)
        if item:
            clipboard = Gtk.Clipboard.get_default(self.get_display())
            clipboard.set_text(item['text'], -1)
            clipboard.store()


class STTTrayApp:
    def __init__(self):
        self.config = Config()
        setup_logging(self.config)
        self.logger = logging.getLogger(__name__)

        self.history = HistoryManager()

        self.is_recording = False
        self.is_listening = False
        self.is_processing = False
        self.running = True

        self.cursor_indicator = CursorIndicator()

        self.controller = SpeechToTextController(
            config=self.config,
            on_state=self._on_state,
            on_transcription=self._on_transcription,
            on_error=self._on_error,
        )

        self.indicator = AppIndicator.Indicator.new(
            'speech-to-text',
            ICON_PAUSED,
            AppIndicator.IndicatorCategory.APPLICATION_STATUS,
        )
        self.indicator.set_status(AppIndicator.IndicatorStatus.ACTIVE)
        self.indicator.set_menu(self._build_menu())

    def _build_menu(self) -> Gtk.Menu:
        menu = Gtk.Menu()

        self.status_item = Gtk.MenuItem(label='○ Paused')
        self.status_item.connect('activate', self._on_toggle)
        menu.append(self.status_item)

        menu.append(Gtk.SeparatorMenuItem())

        settings_item = Gtk.MenuItem(label='Settings')
        settings_item.connect('activate', self._on_settings)
        menu.append(settings_item)

        history_item = Gtk.MenuItem(label='History')
        history_item.connect('activate', self._on_history)
        menu.append(history_item)

        menu.append(Gtk.SeparatorMenuItem())

        quit_item = Gtk.MenuItem(label='Quit')
        quit_item.connect('activate', self._on_quit)
        menu.append(quit_item)

        menu.show_all()
        return menu

    def _update_ui(self) -> None:
        if self.is_processing:
            self.indicator.set_icon_full(ICON_PROCESSING, 'Processing')
            self.status_item.set_label('◐ Processing...')
        elif self.is_recording:
            self.indicator.set_icon_full(ICON_RECORDING, 'Recording')
            self.status_item.set_label('● Recording...')
        elif self.is_listening:
            self.indicator.set_icon_full(ICON_LISTENING, 'Listening')
            self.status_item.set_label('● Listening')
        else:
            self.indicator.set_icon_full(ICON_PAUSED, 'Paused')
            self.status_item.set_label('○ Paused')

        if self.config.get('ui', 'cursor_indicator', default=True):
            if self.is_processing:
                self.cursor_indicator.set_processing()
            elif self.is_recording:
                self.cursor_indicator.set_recording()
            else:
                self.cursor_indicator.set_idle()
        else:
            self.cursor_indicator.set_idle()

    def _on_state(self, is_listening: bool, is_recording: bool, is_processing: bool) -> None:
        self.is_listening = is_listening
        self.is_recording = is_recording
        self.is_processing = is_processing
        GLib.idle_add(self._update_ui)

    def _on_transcription(self, text: str, duration: float) -> None:
        try:
            self.history.add(text)
        except Exception:
            pass

    def _on_error(self, message: str) -> None:
        self.logger.error(message)

    def _on_toggle(self, _widget) -> None:
        self.controller.set_listening(not self.is_listening)

    def _on_settings(self, _widget) -> None:
        dialog = SettingsDialog(None, self.config)
        response = dialog.run()
        if response == Gtk.ResponseType.OK:
            dialog.save()
            self.controller.reload_transcriber()
            self.logger.info('Settings saved, transcriber reloaded')
            self._update_ui()
        dialog.destroy()

    def _on_history(self, _widget) -> None:
        dialog = HistoryDialog(None, self.history)
        dialog.run()
        dialog.destroy()

    def _on_quit(self, _widget) -> None:
        self.running = False
        self.controller.stop()
        try:
            self.cursor_indicator.destroy()
        except Exception:
            pass
        Gtk.main_quit()

    def run(self) -> None:
        self.controller.start()
        self.logger.info('STT Tray App started (GTK/AppIndicator)')
        Gtk.main()


def main():
    signal.signal(signal.SIGINT, signal.SIG_DFL)

    lock_fd = acquire_lock()
    app = STTTrayApp()
    try:
        app.run()
    finally:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        lock_fd.close()


if __name__ == '__main__':
    main()
