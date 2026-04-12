#!/usr/bin/env python3
"""
Speech-to-Text System Tray — GTK/AppIndicator client.

Connects to stt_daemon via Unix socket at /tmp/stt_daemon.sock.
Does NOT run any hotkey listener, audio capture, or transcription itself —
those live in the daemon process, which has no GTK and never appears in the
GNOME taskbar or steals focus.
"""

import glob
import json
import logging
import os
import selectors
import signal
import socket
import sys
import threading
from pathlib import Path

import gi

gi.require_version('Gtk', '3.0')
gi.require_version('Gio', '2.0')
from gi.repository import GLib, Gtk, Gio

try:
    gi.require_version('AppIndicator3', '0.1')
    from gi.repository import AppIndicator3 as AppIndicator
except ValueError:
    gi.require_version('AyatanaAppIndicator3', '0.1')
    from gi.repository import AyatanaAppIndicator3 as AppIndicator

sys.path.insert(0, str(Path(__file__).parent))

from stt_core.config import Config
from stt_gui.history_manager import HistoryManager

SOCKET_PATH = '/tmp/stt_daemon.sock'

ICON_LISTENING   = 'audio-input-microphone'
ICON_PAUSED      = 'microphone-sensitivity-muted'
ICON_RECORDING   = 'media-record'
ICON_PROCESSING  = 'emblem-synchronizing'
ICON_DISCONNECTED = 'network-error'

_KEY_FRIENDLY: dict = {
    **{f'KEY_F{i}': f'F{i}' for i in range(1, 25)},
    'KEY_RIGHTCTRL': 'Right Ctrl',   'KEY_LEFTCTRL': 'Left Ctrl',
    'KEY_RIGHTALT':  'Right Alt',    'KEY_LEFTALT':  'Left Alt',
    'KEY_RIGHTSHIFT':'Right Shift',  'KEY_LEFTSHIFT':'Left Shift',
    'KEY_CAPSLOCK':  'Caps Lock',    'KEY_TAB':      'Tab',
    'KEY_INSERT':    'Insert',       'KEY_DELETE':   'Delete',
    'KEY_HOME':      'Home',         'KEY_END':      'End',
    'KEY_PAGEUP':    'Page Up',      'KEY_PAGEDOWN': 'Page Down',
    'KEY_PAUSE':     'Pause/Break',  'KEY_SYSRQ':    'Print Screen',
    'KEY_SCROLLLOCK':'Scroll Lock',  'KEY_NUMLOCK':  'Num Lock',
    'KEY_BACKSPACE': 'Backspace',    'KEY_ENTER':    'Enter',
}


def _friendly(key_name: str) -> str:
    return _KEY_FRIENDLY.get(key_name,
           key_name.replace('KEY_', '').replace('_', ' ').title())


# ── Daemon IPC client ────────────────────────────────────────────────────────

class DaemonClient:
    """
    Background thread that keeps a persistent connection to stt_daemon.
    Calls on_message(dict) on the GTK main thread via GLib.idle_add.
    Reconnects automatically if the socket disappears.
    """

    def __init__(self, socket_path: str, on_message):
        self.socket_path = socket_path
        self.on_message = on_message
        self._running = True
        self._sock: socket.socket | None = None
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._loop, daemon=True)

    def start(self):
        self._thread.start()

    def stop(self):
        self._running = False
        self._close()

    def send(self, msg: dict) -> None:
        data = (json.dumps(msg) + '\n').encode()
        with self._lock:
            if self._sock:
                try:
                    self._sock.sendall(data)
                except Exception:
                    self._close()

    # ── internal ──────────────────────────────────────────────────────────

    def _close(self):
        with self._lock:
            if self._sock:
                try:
                    self._sock.close()
                except Exception:
                    pass
                self._sock = None

    def _loop(self):
        import time
        while self._running:
            try:
                self._connect_and_read()
            except Exception:
                pass
            if self._running:
                GLib.idle_add(self.on_message, {'type': 'disconnected'})
                time.sleep(2)

    def _connect_and_read(self):
        import time
        while self._running:
            if os.path.exists(self.socket_path):
                break
            time.sleep(1)

        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(5)
        sock.connect(self.socket_path)
        sock.settimeout(None)
        with self._lock:
            self._sock = sock

        GLib.idle_add(self.on_message, {'type': 'connected'})

        buf = b''
        while self._running:
            chunk = sock.recv(4096)
            if not chunk:
                break
            buf += chunk
            while b'\n' in buf:
                line, buf = buf.split(b'\n', 1)
                line = line.strip()
                if line:
                    try:
                        msg = json.loads(line)
                        GLib.idle_add(self.on_message, msg)
                    except json.JSONDecodeError:
                        pass

        self._close()


# ── Key-capture dialog ───────────────────────────────────────────────────────

class KeyCaptureDialog(Gtk.Dialog):
    """Modal dialog: listens to all /dev/input/event* and captures first keydown."""

    def __init__(self, parent):
        super().__init__(title='Capture Hotkey', transient_for=parent,
                         modal=True, flags=0)
        self.set_default_size(340, 160)
        self.add_button('Cancel', Gtk.ResponseType.CANCEL)
        self._ok_btn = self.add_button('Use This Key', Gtk.ResponseType.OK)
        self._ok_btn.set_sensitive(False)

        box = self.get_content_area()
        box.set_spacing(12)
        box.set_margin_top(16); box.set_margin_bottom(12)
        box.set_margin_start(16); box.set_margin_end(16)

        self._prompt = Gtk.Label(label='Press any key on any keyboard…')
        self._prompt.set_xalign(0)
        box.pack_start(self._prompt, False, False, 0)

        self._key_label = Gtk.Label(label='')
        self._key_label.set_xalign(0)
        box.pack_start(self._key_label, False, False, 0)

        self._dev_label = Gtk.Label(label='')
        self._dev_label.set_xalign(0)
        self._dev_label.get_style_context().add_class('dim-label')
        box.pack_start(self._dev_label, False, False, 0)

        self.show_all()
        self._result = None
        self._listening = True
        threading.Thread(target=self._listen, daemon=True).start()

    def _listen(self):
        from evdev import InputDevice, ecodes
        devs = []
        for path in sorted(glob.glob('/dev/input/event*')):
            try:
                d = InputDevice(path)
                if ecodes.EV_KEY in d.capabilities(verbose=False):
                    devs.append(d)
            except Exception:
                pass
        if not devs:
            GLib.idle_add(self._no_devices)
            return

        sel = selectors.DefaultSelector()
        for d in devs:
            sel.register(d, selectors.EVENT_READ)
        try:
            while self._listening:
                for key, _ in sel.select(timeout=0.1):
                    dev = key.fileobj
                    try:
                        for event in dev.read():
                            if event.type == ecodes.EV_KEY and event.value == 1:
                                names = ecodes.KEY.get(event.code, f'KEY_{event.code}')
                                key_name = names[0] if isinstance(names, list) else names
                                self._result = (key_name, dev.path, dev.name)
                                self._listening = False
                                GLib.idle_add(self._on_captured)
                                return
                    except Exception:
                        pass
        finally:
            sel.close()
            for d in devs:
                try: d.close()
                except Exception: pass

    def _on_captured(self):
        key_name, _, dev_name = self._result
        self._prompt.set_markup('<b>Key captured!</b> Click "Use This Key" to confirm.')
        self._key_label.set_markup(
            f'  Key:    <b>{_friendly(key_name)}</b>  <small>({key_name})</small>')
        self._dev_label.set_markup(f'  Device: <small>{dev_name}</small>')
        self._ok_btn.set_sensitive(True)
        return False

    def _no_devices(self):
        self._prompt.set_text('No readable input devices found (permission error?)')
        return False

    def get_result(self):
        return self._result

    def destroy(self):
        self._listening = False
        super().destroy()


# ── Settings dialog ──────────────────────────────────────────────────────────

class SettingsDialog(Gtk.Dialog):
    def __init__(self, parent, config: Config, daemon_client: DaemonClient):
        super().__init__(title='Speech-to-Text Settings',
                         transient_for=parent, flags=0)
        self.config = config
        self.daemon_client = daemon_client
        self.set_default_size(500, 420)
        self.add_buttons(Gtk.STOCK_CANCEL, Gtk.ResponseType.CANCEL,
                         Gtk.STOCK_SAVE,   Gtk.ResponseType.OK)

        self._captured_key    = str(self.config.get('input', 'trigger_key',  default='') or '')
        self._captured_device = str(self.config.get('input', 'device_path',  default='') or '')

        nb = Gtk.Notebook()
        self.get_content_area().pack_start(nb, True, True, 0)
        nb.append_page(self._build_transcription_tab(), Gtk.Label(label='Transcription'))
        nb.append_page(self._build_input_tab(),         Gtk.Label(label='Input'))
        nb.append_page(self._build_output_tab(),        Gtk.Label(label='Output'))
        self.show_all()

    def _build_transcription_tab(self):
        grid = Gtk.Grid(column_spacing=12, row_spacing=8)
        grid.set_margin_top(12); grid.set_margin_bottom(12)
        grid.set_margin_start(12); grid.set_margin_end(12)
        row = 0

        grid.attach(Gtk.Label(label='Model:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.model_combo = Gtk.ComboBoxText()
        for m in ('tiny.en', 'base.en', 'small.en', 'medium.en', 'large-v3'):
            self.model_combo.append(m, m)
        cur = str(self.config.get('transcription', 'model', default='base.en'))
        if not self.model_combo.set_active_id(cur):
            self.model_combo.set_active(1)
        grid.attach(self.model_combo, 1, row, 1, 1); row += 1

        grid.attach(Gtk.Label(label='Compute type:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.compute_combo = Gtk.ComboBoxText()
        for c in ('int8', 'int8_float16', 'float16', 'float32'):
            self.compute_combo.append(c, c)
        cur = str(self.config.get('transcription', 'compute_type', default='int8'))
        if not self.compute_combo.set_active_id(cur):
            self.compute_combo.set_active(0)
        grid.attach(self.compute_combo, 1, row, 1, 1); row += 1

        grid.attach(Gtk.Label(label='Language:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.lang_entry = Gtk.Entry()
        self.lang_entry.set_text(str(self.config.get('transcription', 'language', default='en')))
        grid.attach(self.lang_entry, 1, row, 1, 1); row += 1

        grid.attach(Gtk.Label(label='Accuracy (beam size):', halign=Gtk.Align.START), 0, row, 1, 1)
        self.beam_spin = Gtk.SpinButton.new_with_range(1, 10, 1)
        self.beam_spin.set_value(int(self.config.get('transcription', 'beam_size', default=1) or 1))
        grid.attach(self.beam_spin, 1, row, 1, 1); row += 1

        self.vad_check = Gtk.CheckButton(label='Remove silence (VAD)')
        self.vad_check.set_active(bool(self.config.get('transcription', 'vad_filter', default=True)))
        grid.attach(self.vad_check, 1, row, 1, 1)
        return grid

    def _build_input_tab(self):
        grid = Gtk.Grid(column_spacing=12, row_spacing=10)
        grid.set_margin_top(16); grid.set_margin_bottom(16)
        grid.set_margin_start(16); grid.set_margin_end(16)
        row = 0

        grid.attach(Gtk.Label(label='Hotkey:', halign=Gtk.Align.START), 0, row, 1, 1)
        hbox = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        self._key_badge = Gtk.Label(); self._key_badge.set_xalign(0)
        self._refresh_key_badge()
        hbox.pack_start(self._key_badge, True, True, 0)
        btn = Gtk.Button(label='Capture Key…')
        btn.set_tooltip_text('Click then press any key to use as the hotkey')
        btn.connect('clicked', self._on_capture_clicked)
        hbox.pack_start(btn, False, False, 0)
        grid.attach(hbox, 1, row, 1, 1); row += 1

        grid.attach(Gtk.Label(label='Device:', halign=Gtk.Align.START), 0, row, 1, 1)
        self._device_label = Gtk.Label(halign=Gtk.Align.START)
        self._device_label.set_line_wrap(True)
        self._refresh_device_label()
        grid.attach(self._device_label, 1, row, 1, 1); row += 1

        hint = Gtk.Label()
        hint.set_markup('<small><i>Hold the hotkey to record, release to transcribe.</i></small>')
        hint.set_xalign(0)
        grid.attach(hint, 0, row, 2, 1)
        return grid

    def _refresh_key_badge(self):
        k = self._captured_key
        if k:
            self._key_badge.set_markup(f'<b>{_friendly(k)}</b>  <small>({k})</small>')
        else:
            self._key_badge.set_markup('<i>Not set — click Capture Key…</i>')

    def _refresh_device_label(self):
        d = self._captured_device
        self._device_label.set_markup(
            f'<small>{d}</small>' if d else '<small><i>Auto-detect</i></small>')

    def _on_capture_clicked(self, _w):
        dlg = KeyCaptureDialog(self)
        if dlg.run() == Gtk.ResponseType.OK:
            res = dlg.get_result()
            if res:
                self._captured_key, self._captured_device, _ = res
                self._refresh_key_badge()
                self._refresh_device_label()
        dlg.destroy()

    def _build_output_tab(self):
        grid = Gtk.Grid(column_spacing=12, row_spacing=8)
        grid.set_margin_top(12); grid.set_margin_bottom(12)
        grid.set_margin_start(12); grid.set_margin_end(12)
        row = 0

        grid.attach(Gtk.Label(label='Output method:', halign=Gtk.Align.START), 0, row, 1, 1)
        self.out_combo = Gtk.ComboBoxText()
        for o in ('auto', 'xdotool', 'ydotool', 'dotool', 'wtype', 'xclip', 'none'):
            self.out_combo.append(o, o)
        cur = str(self.config.get('output', 'method', default='auto'))
        if not self.out_combo.set_active_id(cur):
            self.out_combo.set_active(0)
        grid.attach(self.out_combo, 1, row, 1, 1); row += 1

        self.space_check = Gtk.CheckButton(label='Add space after text')
        self.space_check.set_active(bool(self.config.get('output', 'add_space', default=True)))
        grid.attach(self.space_check, 1, row, 1, 1)
        return grid

    def save(self):
        for section in ('transcription', 'input', 'output', 'ui'):
            self.config.data.setdefault(section, {})
        self.config.data['transcription'].update({
            'model':        self.model_combo.get_active_text() or 'base.en',
            'compute_type': self.compute_combo.get_active_text() or 'int8',
            'language':     self.lang_entry.get_text() or 'en',
            'beam_size':    int(self.beam_spin.get_value()),
            'vad_filter':   self.vad_check.get_active(),
        })
        self.config.data['input'].update({
            'trigger_key': self._captured_key,
            'device_path': self._captured_device,
        })
        self.config.data['output'].update({
            'method':    self.out_combo.get_active_text() or 'auto',
            'add_space': self.space_check.get_active(),
        })
        self.config.save()
        self.daemon_client.send({'cmd': 'reload_config'})


# ── History dialog ───────────────────────────────────────────────────────────

class HistoryDialog(Gtk.Dialog):
    def __init__(self, parent, history: HistoryManager):
        super().__init__(title='Transcription History', transient_for=parent, flags=0)
        self.history = history
        self.set_default_size(600, 400)
        self.add_buttons(Gtk.STOCK_CLOSE, Gtk.ResponseType.CLOSE)

        self.store = Gtk.ListStore(int, str, str)
        self.tree  = Gtk.TreeView(model=self.store)
        r = Gtk.CellRendererText()
        c1 = Gtk.TreeViewColumn('Time', r, text=1); c1.set_min_width(140)
        c2 = Gtk.TreeViewColumn('Transcription', r, text=2); c2.set_expand(True)
        self.tree.append_column(c1); self.tree.append_column(c2)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        scroll.add(self.tree)

        box = self.get_content_area()
        box.pack_start(scroll, True, True, 0)
        btn = Gtk.Button(label='Copy to Clipboard')
        btn.connect('clicked', self._on_copy)
        box.pack_start(btn, False, False, 8)

        self._load()
        self.show_all()

    def _load(self):
        self.store.clear()
        for item in self.history.get_recent(50):
            txt = item['text'][:80] + '...' if len(item['text']) > 80 else item['text']
            self.store.append([item['id'], item['timestamp'], txt])

    def _on_copy(self, _w):
        model, it = self.tree.get_selection().get_selected()
        if it is None: return
        item = self.history.get_by_id(model[it][0])
        if item:
            cb = Gtk.Clipboard.get_default(self.get_display())
            cb.set_text(item['text'], -1); cb.store()


# ── Tray application ─────────────────────────────────────────────────────────

class STTTrayApp(Gtk.Application):
    """
    Pure tray client — no hotkey listener, no audio, no typing.
    Communicates with stt_daemon via Unix socket.
    Uses IS_SERVICE so GNOME never shows it in the taskbar or steals focus.
    """

    def __init__(self):
        super().__init__(
            application_id='com.github.voice-dictation.stt-tray',
            flags=Gio.ApplicationFlags.IS_SERVICE,
        )
        self.config  = Config()
        self.history = HistoryManager()
        self.logger  = logging.getLogger(__name__)

        self.is_listening   = False
        self.is_recording   = False
        self.is_processing  = False
        self.is_connected   = False

        self.indicator: AppIndicator.Indicator | None = None
        self.status_item:  Gtk.MenuItem | None = None
        self.toggle_item:  Gtk.MenuItem | None = None

        self.client = DaemonClient(SOCKET_PATH, self._on_daemon_message)

    # ── GApplication lifecycle ─────────────────────────────────────────────

    def do_startup(self):
        Gtk.Application.do_startup(self)
        self.hold()   # keep alive — IS_SERVICE won't auto-quit

        self.indicator = AppIndicator.Indicator.new(
            'speech-to-text',
            ICON_DISCONNECTED,
            AppIndicator.IndicatorCategory.APPLICATION_STATUS,
        )
        self.indicator.set_status(AppIndicator.IndicatorStatus.ACTIVE)
        self.indicator.set_menu(self._build_menu())

        self.client.start()
        self.logger.info('STT Tray started (IS_SERVICE, connecting to daemon)')

    def do_activate(self):
        pass

    # ── Menu ───────────────────────────────────────────────────────────────

    def _build_menu(self):
        menu = Gtk.Menu()

        self.status_item = Gtk.MenuItem(label='⊘ Connecting…')
        self.status_item.set_sensitive(False)
        menu.append(self.status_item)

        self.toggle_item = Gtk.MenuItem(label='Enable')
        self.toggle_item.connect('activate', self._on_toggle)
        self.toggle_item.set_sensitive(False)
        menu.append(self.toggle_item)

        menu.append(Gtk.SeparatorMenuItem())

        settings_item = Gtk.MenuItem(label='Settings…')
        settings_item.connect('activate', self._on_settings)
        menu.append(settings_item)

        history_item = Gtk.MenuItem(label='History…')
        history_item.connect('activate', self._on_history)
        menu.append(history_item)

        menu.append(Gtk.SeparatorMenuItem())

        quit_item = Gtk.MenuItem(label='Quit Tray')
        quit_item.connect('activate', self._on_quit)
        menu.append(quit_item)

        menu.show_all()
        return menu

    # ── Daemon message handler (GTK main thread via GLib.idle_add) ─────────

    def _on_daemon_message(self, msg: dict):
        t = msg.get('type')
        if t == 'connected':
            self.is_connected = True
            self.toggle_item.set_sensitive(True)
        elif t == 'disconnected':
            self.is_connected   = False
            self.is_listening   = False
            self.is_recording   = False
            self.is_processing  = False
            self.toggle_item.set_sensitive(False)
        elif t == 'state':
            self.is_listening  = msg.get('listening',  False)
            self.is_recording  = msg.get('recording',  False)
            self.is_processing = msg.get('processing', False)
        elif t == 'transcription':
            try:
                self.history.add(msg.get('text', ''))
            except Exception:
                pass
        elif t == 'error':
            self.logger.error('Daemon error: %s', msg.get('message'))
        self._update_ui()
        return False  # remove from idle queue

    def _update_ui(self):
        if not self.is_connected:
            self.indicator.set_icon_full(ICON_DISCONNECTED, 'Disconnected')
            self.status_item.set_label('⊘ Daemon not running')
            self.toggle_item.set_label('Enable')
        elif self.is_processing:
            self.indicator.set_icon_full(ICON_PROCESSING, 'Processing')
            self.status_item.set_label('◐ Processing…')
        elif self.is_recording:
            self.indicator.set_icon_full(ICON_RECORDING, 'Recording')
            self.status_item.set_label('● Recording…')
        elif self.is_listening:
            self.indicator.set_icon_full(ICON_LISTENING, 'Listening')
            self.status_item.set_label('● Listening')
            self.toggle_item.set_label('Pause')
        else:
            self.indicator.set_icon_full(ICON_PAUSED, 'Paused')
            self.status_item.set_label('○ Paused')
            self.toggle_item.set_label('Enable')

    # ── Menu callbacks ─────────────────────────────────────────────────────

    def _on_toggle(self, _w):
        self.client.send({'cmd': 'set_listening', 'value': not self.is_listening})

    def _on_settings(self, _w):
        dlg = SettingsDialog(None, self.config, self.client)
        if dlg.run() == Gtk.ResponseType.OK:
            dlg.save()
            self.logger.info('Settings saved and reload_config sent to daemon')
        dlg.destroy()

    def _on_history(self, _w):
        dlg = HistoryDialog(None, self.history)
        dlg.run()
        dlg.destroy()

    def _on_quit(self, _w):
        self.client.stop()
        self.quit()


# ── Entry point ──────────────────────────────────────────────────────────────

def main():
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    app = STTTrayApp()
    sys.exit(app.run(sys.argv))


if __name__ == '__main__':
    main()
