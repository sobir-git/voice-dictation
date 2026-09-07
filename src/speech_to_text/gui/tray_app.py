import logging
import threading

from speech_to_text.core.app_logging import setup_logging
from speech_to_text.core.config import Config
from speech_to_text.core.diagnostics import microphones
from speech_to_text.core.runtime import SOCKET_PATH, ensure_daemon
from speech_to_text.gui.gtk import AppIndicator, Gio, GLib, Gtk
from speech_to_text.gui.history_manager import HistoryManager
from speech_to_text.gui.ipc_client import DaemonClient

logger = logging.getLogger(__name__)


class STTTrayApp(Gtk.Application):
    def __init__(self):
        super().__init__(application_id='com.github.voice-dictation.stt-tray',
                         flags=Gio.ApplicationFlags.HANDLES_COMMAND_LINE)
        self.add_main_option('background', 0, GLib.OptionFlags.NONE, GLib.OptionArg.NONE, 'Start in the tray', None)
        self.config = Config()
        setup_logging(self.config, 'tray')
        self.history = HistoryManager()
        self.is_listening = self.is_recording = self.is_processing = self.is_connected = False
        self.model_ready = False
        self.capture_ready = False
        self.cancel_requested = False
        self.last_error = ''
        self.microphone_name = ''
        self.window = None
        self.indicator = None
        self.hud = None
        self.client = DaemonClient(SOCKET_PATH, self._on_daemon_message, reconnect=ensure_daemon)

    def do_startup(self):
        Gtk.Application.do_startup(self)
        self.hold()
        self.indicator = AppIndicator.Indicator.new('speech-to-text', 'audio-input-microphone',
                                                   AppIndicator.IndicatorCategory.APPLICATION_STATUS)
        self.indicator.set_status(AppIndicator.IndicatorStatus.ACTIVE)
        menu = Gtk.Menu()
        self.status_item = self._menu_item(menu, 'Connecting…')
        self.status_item.set_sensitive(False)
        self._menu_item(menu, 'Open Voice Dictation', lambda *_: self.open_window())
        self.toggle_item = self._menu_item(menu, 'Enable listening', self._on_toggle)
        menu.append(Gtk.SeparatorMenuItem())
        self._menu_item(menu, 'History', lambda *_: self.open_window('history'))
        self._menu_item(menu, 'Settings', lambda *_: self.open_window('settings'))
        self._menu_item(menu, 'Diagnostics', lambda *_: self.open_window('diagnostics'))
        self._menu_item(menu, 'Cancel dictation', lambda *_: self.client.send({'cmd': 'cancel'}))
        menu.append(Gtk.SeparatorMenuItem())
        self._menu_item(menu, 'Quit tray', lambda *_: self.quit())
        menu.show_all()
        self.indicator.set_menu(menu)
        self.client.start()
        self._load_microphone_name()
        logger.info('Tray started')

    def do_activate(self):
        self.open_window()

    def do_command_line(self, command_line):
        if '--background' not in command_line.get_arguments():
            self.open_window()
        return 0

    def do_shutdown(self):
        self.client.stop()
        if self.window:
            self.window.destroy()
        if self.hud:
            self.hud.destroy()
        Gtk.Application.do_shutdown(self)

    @staticmethod
    def _menu_item(menu, title, callback=None):
        item = Gtk.MenuItem(label=title)
        if callback:
            item.connect('activate', callback)
        menu.append(item)
        return item

    def open_window(self, page='dictation'):
        if self.window is None:
            from speech_to_text.gui.main_window import DictationWindow
            self.window = DictationWindow(self)
        self.window.show_page(page)
        if self.last_error:
            self.window.show_error(self.last_error)
        self.window.show()
        self.window.present()

    def _load_microphone_name(self):
        def load():
            node = self.config.get('audio', 'pipewire_node')
            name = next((m['description'] for m in microphones() if m['name'] == node), '')
            GLib.idle_add(self._set_microphone_name, name)
        threading.Thread(target=load, name='microphone-name', daemon=True).start()

    def _set_microphone_name(self, name):
        self.microphone_name = name
        if self.window:
            self.window.update_state()
        return False

    def _on_daemon_message(self, msg):
        kind = msg.get('type')
        if kind == 'connected':
            self.is_connected = True
        elif kind == 'disconnected':
            self.is_connected = self.is_listening = self.is_recording = self.is_processing = False
        elif kind == 'state':
            self.is_listening = msg.get('listening', False)
            self.is_recording = msg.get('recording', False)
            self.is_processing = msg.get('processing', False)
            self.model_ready = msg.get('model_ready', False)
            self.capture_ready = msg.get('capture_ready', False)
            if self.is_recording or not self.is_processing:
                self.cancel_requested = False
            if msg.get('last_error'):
                self.last_error = msg['last_error']
        elif kind == 'audio_level':
            was_ready = self.capture_ready
            self.capture_ready = True
            if self.window:
                self.window.signal.level = msg.get('level', 0)
                self.window.signal.queue_draw()
            if not was_ready:
                self._update_ui()
            return False
        elif kind == 'cancelled':
            self.cancel_requested = self.is_processing
        elif kind == 'transcription':
            if self.window:
                self.window.transcription(msg.get('text', ''), msg.get('duration', 0))
        elif kind == 'config_reloaded':
            self.config.load()
            setup_logging(self.config, 'tray')
            self._load_microphone_name()
            if self.window:
                self.window.save_status.set_text('Changes applied.')
        elif kind == 'error':
            self.last_error = msg.get('message', 'Unknown error')
            logger.error('Daemon: %s', self.last_error)
            if self.window:
                self.window.save_status.set_text(self.last_error)
        self._update_ui()
        return False

    def _update_ui(self):
        if not self.is_connected:
            icon, text = 'network-error', 'Reconnecting…'
        elif self.is_recording:
            icon, text = 'media-record', 'Recording' if self.capture_ready else 'Opening microphone…'
        elif self.is_processing:
            icon, text = 'emblem-synchronizing', 'Transcribing…'
        elif self.is_listening:
            icon, text = 'audio-input-microphone', 'Ready' if self.model_ready else 'Loading model…'
        else:
            icon, text = 'microphone-sensitivity-muted', 'Paused'
        self.indicator.set_icon_full(icon, text)
        self.status_item.set_label(text)
        self.toggle_item.set_label('Pause listening' if self.is_listening else 'Enable listening')
        self.toggle_item.set_sensitive(self.is_connected)
        if self.window:
            self.window.update_state()
            if self.last_error:
                self.window.show_error(self.last_error)
        self._update_hud(text)

    def _update_hud(self, text):
        show = self.config.get('ui', 'cursor_indicator') and (self.is_recording or self.is_processing)
        if not show:
            if self.hud:
                self.hud.hide()
            return
        if self.hud is None:
            self.hud = Gtk.Window(type=Gtk.WindowType.POPUP)
            self.hud.get_style_context().add_class('dictation')
            self.hud.set_accept_focus(False)
            self.hud.set_focus_on_map(False)
            self.hud.set_keep_above(True)
            self.hud.set_skip_taskbar_hint(True)
            self.hud_label = Gtk.Label()
            self.hud_label.set_margin_top(12)
            self.hud_label.set_margin_bottom(12)
            self.hud_label.set_margin_start(20)
            self.hud_label.set_margin_end(20)
            self.hud.add(self.hud_label)
            self.hud.set_position(Gtk.WindowPosition.CENTER)
        self.hud_label.set_text(('● ' if self.is_recording else '◌ ') + text)
        if not self.hud.get_visible():
            display = self.hud.get_display()
            pointer = display.get_default_seat().get_pointer()
            _, x, y = pointer.get_position()
            self.hud.move(x + 20, y + 24)
        self.hud.show_all()

    def _on_toggle(self, *_):
        self.client.send({'cmd': 'set_listening', 'value': not self.is_listening})
