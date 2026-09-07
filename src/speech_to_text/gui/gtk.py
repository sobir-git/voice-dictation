import gi

gi.require_version('Gtk', '3.0')
gi.require_version('Gio', '2.0')
from gi.repository import GLib, Gtk, Gio

# Voice Dictation always uses dark native controls, including popup dialogs.
_settings = Gtk.Settings.get_default()
if _settings is not None:
    _settings.set_property('gtk-application-prefer-dark-theme', True)

try:
    gi.require_version('AppIndicator3', '0.1')
    from gi.repository import AppIndicator3 as AppIndicator
except ValueError:
    gi.require_version('AyatanaAppIndicator3', '0.1')
    from gi.repository import AyatanaAppIndicator3 as AppIndicator
