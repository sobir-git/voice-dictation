import logging

from speech_to_text.core.config import Config
from speech_to_text.gui.gtk import AppIndicator, Gio, Gtk
from speech_to_text.gui.history_dialog import HistoryDialog
from speech_to_text.gui.history_manager import HistoryManager
from speech_to_text.gui.ipc_client import DaemonClient, SOCKET_PATH
from speech_to_text.gui.settings_dialog import SettingsDialog

ICON_LISTENING = 'audio-input-microphone'
ICON_PAUSED = 'microphone-sensitivity-muted'
ICON_RECORDING = 'media-record'
ICON_PROCESSING = 'emblem-synchronizing'
ICON_DISCONNECTED = 'network-error'


class STTTrayApp(Gtk.Application):
    """Pure tray client for the daemon process."""

    def __init__(self):
        super().__init__(
            application_id='com.github.voice-dictation.stt-tray',
            flags=Gio.ApplicationFlags.IS_SERVICE,
        )
        self.config = Config()
        self.history = HistoryManager()
        self.logger = logging.getLogger(__name__)

        self.is_listening = False
        self.is_recording = False
        self.is_processing = False
        self.is_connected = False

        self.indicator: AppIndicator.Indicator | None = None
        self.status_item: Gtk.MenuItem | None = None
        self.toggle_item: Gtk.MenuItem | None = None

        self.client = DaemonClient(SOCKET_PATH, self._on_daemon_message)

    def do_startup(self):
        Gtk.Application.do_startup(self)
        self.hold()

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

    def _build_menu(self):
        menu = Gtk.Menu()
        self.status_item = self._build_menu_item('⊘ Connecting...', sensitive=False)
        self.toggle_item = self._build_menu_item('Enable', self._on_toggle, sensitive=False)
        menu.append(self.status_item)
        menu.append(self.toggle_item)

        menu.append(Gtk.SeparatorMenuItem())

        menu.append(self._build_menu_item('Settings...', self._on_settings))
        menu.append(self._build_menu_item('History...', self._on_history))

        menu.append(Gtk.SeparatorMenuItem())
        menu.append(self._build_menu_item('Quit Tray', self._on_quit))

        menu.show_all()
        return menu

    def _build_menu_item(self, label: str, callback=None, sensitive: bool = True) -> Gtk.MenuItem:
        item = Gtk.MenuItem(label=label)
        item.set_sensitive(sensitive)
        if callback is not None:
            item.connect('activate', callback)
        return item

    def _on_daemon_message(self, msg: dict):
        msg_type = msg.get('type')
        if msg_type == 'connected':
            self.is_connected = True
            self.toggle_item.set_sensitive(True)
        elif msg_type == 'disconnected':
            self.is_connected = False
            self.is_listening = False
            self.is_recording = False
            self.is_processing = False
            self.toggle_item.set_sensitive(False)
        elif msg_type == 'state':
            self.is_listening = msg.get('listening', False)
            self.is_recording = msg.get('recording', False)
            self.is_processing = msg.get('processing', False)
        elif msg_type == 'transcription':
            try:
                self.history.add(msg.get('text', ''))
            except Exception:
                pass
        elif msg_type == 'error':
            self.logger.error('Daemon error: %s', msg.get('message'))

        self._update_ui()
        return False

    def _update_ui(self) -> None:
        if not self.is_connected:
            self._set_status(ICON_DISCONNECTED, 'Disconnected', '⊘ Daemon not running', 'Enable')
            return

        if self.is_processing:
            self._set_status(ICON_PROCESSING, 'Processing', '◐ Processing...')
            return

        if self.is_recording:
            self._set_status(ICON_RECORDING, 'Recording', '● Recording...')
            return

        if self.is_listening:
            self._set_status(ICON_LISTENING, 'Listening', '● Listening', 'Pause')
            return

        self._set_status(ICON_PAUSED, 'Paused', '○ Paused', 'Enable')

    def _set_status(self, icon: str, title: str, status_label: str, toggle_label: str | None = None) -> None:
        self.indicator.set_icon_full(icon, title)
        self.status_item.set_label(status_label)
        if toggle_label is not None:
            self.toggle_item.set_label(toggle_label)

    def _on_toggle(self, _widget) -> None:
        self.client.send({'cmd': 'set_listening', 'value': not self.is_listening})

    def _on_settings(self, _widget) -> None:
        dialog = SettingsDialog(None, self.config, self.client)
        if dialog.run() == Gtk.ResponseType.OK:
            dialog.save()
            self.logger.info('Settings saved and reload_config sent to daemon')
        dialog.destroy()

    def _on_history(self, _widget) -> None:
        dialog = HistoryDialog(None, self.history)
        dialog.run()
        dialog.destroy()

    def _on_quit(self, _widget) -> None:
        self.client.stop()
        self.quit()
