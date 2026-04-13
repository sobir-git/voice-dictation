import glob
import selectors
import threading
from typing import TYPE_CHECKING

from speech_to_text.core.config import Config
from speech_to_text.gui.gtk import GLib, Gtk

if TYPE_CHECKING:
    from speech_to_text.gui.ipc_client import DaemonClient
KEY_FRIENDLY: dict[str, str] = {
    **{f'KEY_F{i}': f'F{i}' for i in range(1, 25)},
    'KEY_RIGHTCTRL': 'Right Ctrl',
    'KEY_LEFTCTRL': 'Left Ctrl',
    'KEY_RIGHTALT': 'Right Alt',
    'KEY_LEFTALT': 'Left Alt',
    'KEY_RIGHTSHIFT': 'Right Shift',
    'KEY_LEFTSHIFT': 'Left Shift',
    'KEY_CAPSLOCK': 'Caps Lock',
    'KEY_TAB': 'Tab',
    'KEY_INSERT': 'Insert',
    'KEY_DELETE': 'Delete',
    'KEY_HOME': 'Home',
    'KEY_END': 'End',
    'KEY_PAGEUP': 'Page Up',
    'KEY_PAGEDOWN': 'Page Down',
    'KEY_PAUSE': 'Pause/Break',
    'KEY_SYSRQ': 'Print Screen',
    'KEY_SCROLLLOCK': 'Scroll Lock',
    'KEY_NUMLOCK': 'Num Lock',
    'KEY_BACKSPACE': 'Backspace',
    'KEY_ENTER': 'Enter',
}

MODEL_OPTIONS = ('tiny.en', 'base.en', 'small.en', 'medium.en', 'large-v3')
COMPUTE_OPTIONS = ('int8', 'int8_float16', 'float16', 'float32')
OUTPUT_OPTIONS = ('auto', 'xdotool', 'ydotool', 'dotool', 'wtype', 'xclip', 'none')


def friendly_key_name(key_name: str) -> str:
    return KEY_FRIENDLY.get(key_name, key_name.replace('KEY_', '').replace('_', ' ').title())


class KeyCaptureDialog(Gtk.Dialog):
    """Modal dialog that captures the next readable key press."""

    def __init__(self, parent):
        super().__init__(title='Capture Hotkey', transient_for=parent, modal=True, flags=0)
        self.set_default_size(340, 160)
        self.add_button('Cancel', Gtk.ResponseType.CANCEL)
        self._ok_button = self.add_button('Use This Key', Gtk.ResponseType.OK)
        self._ok_button.set_sensitive(False)

        box = self.get_content_area()
        box.set_spacing(12)
        for setter in (box.set_margin_top, box.set_margin_start):
            setter(16)
        for setter in (box.set_margin_bottom, box.set_margin_end):
            setter(12)

        self._prompt = self._new_label('Press any key on any keyboard...')
        box.pack_start(self._prompt, False, False, 0)

        self._key_label = self._new_label('')
        box.pack_start(self._key_label, False, False, 0)

        self._device_label = self._new_label('')
        self._device_label.get_style_context().add_class('dim-label')
        box.pack_start(self._device_label, False, False, 0)

        self._result = None
        self._listening = True
        self.show_all()
        threading.Thread(target=self._listen, daemon=True).start()

    def _new_label(self, text: str) -> Gtk.Label:
        label = Gtk.Label(label=text)
        label.set_xalign(0)
        return label

    def _listen(self) -> None:
        from evdev import InputDevice, ecodes

        devices = []
        for path in sorted(glob.glob('/dev/input/event*')):
            try:
                device = InputDevice(path)
                if ecodes.EV_KEY in device.capabilities(verbose=False):
                    devices.append(device)
            except Exception:
                pass
        if not devices:
            GLib.idle_add(self._no_devices)
            return

        selector = selectors.DefaultSelector()
        for device in devices:
            selector.register(device, selectors.EVENT_READ)

        try:
            while self._listening:
                for key, _ in selector.select(timeout=0.1):
                    device = key.fileobj
                    try:
                        for event in device.read():
                            if event.type != ecodes.EV_KEY or event.value != 1:
                                continue
                            names = ecodes.KEY.get(event.code, f'KEY_{event.code}')
                            key_name = names[0] if isinstance(names, list) else names
                            self._result = (key_name, device.path, device.name)
                            self._listening = False
                            GLib.idle_add(self._on_captured)
                            return
                    except Exception:
                        pass
        finally:
            selector.close()
            for device in devices:
                try:
                    device.close()
                except Exception:
                    pass

    def _on_captured(self):
        key_name, _, device_name = self._result
        self._prompt.set_markup('<b>Key captured!</b> Click "Use This Key" to confirm.')
        self._key_label.set_markup(
            f'  Key:    <b>{friendly_key_name(key_name)}</b>  <small>({key_name})</small>'
        )
        self._device_label.set_markup(f'  Device: <small>{device_name}</small>')
        self._ok_button.set_sensitive(True)
        return False

    def _no_devices(self):
        self._prompt.set_text('No readable input devices found (permission error?)')
        return False

    def get_result(self):
        return self._result

    def destroy(self):
        self._listening = False
        super().destroy()


class SettingsDialog(Gtk.Dialog):
    def __init__(self, parent, config: Config, daemon_client: 'DaemonClient'):
        super().__init__(title='Speech-to-Text Settings', transient_for=parent, flags=0)
        self.config = config
        self.daemon_client = daemon_client
        self.set_default_size(500, 420)
        self.add_buttons(Gtk.STOCK_CANCEL, Gtk.ResponseType.CANCEL, Gtk.STOCK_SAVE, Gtk.ResponseType.OK)
        self._captured_key = str(self.config.get('input', 'trigger_key', default='') or '')
        self._captured_device = str(self.config.get('input', 'device_path', default='') or '')

        notebook = Gtk.Notebook()
        self.get_content_area().pack_start(notebook, True, True, 0)
        notebook.append_page(self._build_transcription_tab(), Gtk.Label(label='Transcription'))
        notebook.append_page(self._build_input_tab(), Gtk.Label(label='Input'))
        notebook.append_page(self._build_output_tab(), Gtk.Label(label='Output'))
        self.show_all()

    def _build_transcription_tab(self) -> Gtk.Grid:
        grid = self._build_grid(12, 8)
        self.model_combo = self._build_combo(
            MODEL_OPTIONS,
            str(self.config.get('transcription', 'model', default='base.en')),
            1,
        )
        self.compute_combo = self._build_combo(
            COMPUTE_OPTIONS,
            str(self.config.get('transcription', 'compute_type', default='int8')),
            0,
        )

        grid.attach(self._new_start_label('Model:'), 0, 0, 1, 1)
        grid.attach(self.model_combo, 1, 0, 1, 1)
        grid.attach(self._new_start_label('Compute type:'), 0, 1, 1, 1)
        grid.attach(self.compute_combo, 1, 1, 1, 1)

        grid.attach(self._new_start_label('Language:'), 0, 2, 1, 1)
        self.lang_entry = Gtk.Entry()
        self.lang_entry.set_text(str(self.config.get('transcription', 'language', default='en')))
        grid.attach(self.lang_entry, 1, 2, 1, 1)

        grid.attach(self._new_start_label('Accuracy (beam size):'), 0, 3, 1, 1)
        self.beam_spin = Gtk.SpinButton.new_with_range(1, 10, 1)
        self.beam_spin.set_value(int(self.config.get('transcription', 'beam_size', default=1) or 1))
        grid.attach(self.beam_spin, 1, 3, 1, 1)

        self.vad_check = Gtk.CheckButton(label='Remove silence (VAD)')
        self.vad_check.set_active(bool(self.config.get('transcription', 'vad_filter', default=True)))
        grid.attach(self.vad_check, 1, 4, 1, 1)
        return grid

    def _build_input_tab(self) -> Gtk.Grid:
        grid = self._build_grid(16, 10)
        box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        self._key_badge = Gtk.Label()
        self._key_badge.set_xalign(0)
        self._refresh_key_badge()
        box.pack_start(self._key_badge, True, True, 0)

        button = Gtk.Button(label='Capture Key...')
        button.set_tooltip_text('Click then press any key to use as the hotkey')
        button.connect('clicked', self._on_capture_clicked)
        box.pack_start(button, False, False, 0)

        grid.attach(self._new_start_label('Hotkey:'), 0, 0, 1, 1)
        grid.attach(box, 1, 0, 1, 1)

        grid.attach(self._new_start_label('Device:'), 0, 1, 1, 1)
        self._device_label = Gtk.Label(halign=Gtk.Align.START)
        self._device_label.set_line_wrap(True)
        self._refresh_device_label()
        grid.attach(self._device_label, 1, 1, 1, 1)

        hint = Gtk.Label()
        hint.set_markup('<small><i>Hold the hotkey to record, release to transcribe.</i></small>')
        hint.set_xalign(0)
        grid.attach(hint, 0, 2, 2, 1)
        return grid

    def _build_output_tab(self) -> Gtk.Grid:
        grid = self._build_grid(12, 8)
        self.out_combo = self._build_combo(
            OUTPUT_OPTIONS,
            str(self.config.get('output', 'method', default='auto')),
            0,
        )
        grid.attach(self._new_start_label('Output method:'), 0, 0, 1, 1)
        grid.attach(self.out_combo, 1, 0, 1, 1)

        self.space_check = Gtk.CheckButton(label='Add space after text')
        self.space_check.set_active(bool(self.config.get('output', 'add_space', default=True)))
        grid.attach(self.space_check, 1, 1, 1, 1)
        return grid

    def _build_grid(self, margin: int, row_spacing: int) -> Gtk.Grid:
        grid = Gtk.Grid(column_spacing=12, row_spacing=row_spacing)
        grid.set_margin_top(margin)
        grid.set_margin_bottom(margin)
        grid.set_margin_start(margin)
        grid.set_margin_end(margin)
        return grid

    def _build_combo(self, values: tuple[str, ...], current_value: str, fallback_index: int) -> Gtk.ComboBoxText:
        combo = Gtk.ComboBoxText()
        for value in values:
            combo.append(value, value)
        if not combo.set_active_id(current_value):
            combo.set_active(fallback_index)
        return combo

    def _new_start_label(self, text: str) -> Gtk.Label:
        return Gtk.Label(label=text, halign=Gtk.Align.START)

    def _refresh_key_badge(self) -> None:
        if self._captured_key:
            self._key_badge.set_markup(
                f'<b>{friendly_key_name(self._captured_key)}</b>  <small>({self._captured_key})</small>'
            )
            return
        self._key_badge.set_markup('<i>Not set - click Capture Key...</i>')

    def _refresh_device_label(self) -> None:
        if self._captured_device:
            self._device_label.set_markup(f'<small>{self._captured_device}</small>')
            return
        self._device_label.set_markup('<small><i>Auto-detect</i></small>')

    def _on_capture_clicked(self, _widget) -> None:
        dialog = KeyCaptureDialog(self)
        if dialog.run() == Gtk.ResponseType.OK:
            result = dialog.get_result()
            if result:
                self._captured_key, self._captured_device, _ = result
                self._refresh_key_badge()
                self._refresh_device_label()
        dialog.destroy()

    def save(self) -> None:
        for section in ('transcription', 'input', 'output'):
            self.config.data.setdefault(section, {})
        self.config.data['transcription'].update(
            {
                'model': self.model_combo.get_active_text() or 'base.en',
                'compute_type': self.compute_combo.get_active_text() or 'int8',
                'language': self.lang_entry.get_text() or 'en',
                'beam_size': int(self.beam_spin.get_value()),
                'vad_filter': self.vad_check.get_active(),
            }
        )
        self.config.data['input'].update({'trigger_key': self._captured_key, 'device_path': self._captured_device})
        self.config.data['output'].update(
            {'method': self.out_combo.get_active_text() or 'auto', 'add_space': self.space_check.get_active()}
        )
        self.config.save()
        self.daemon_client.send({'cmd': 'reload_config'})
