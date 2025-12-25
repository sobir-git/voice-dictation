"""
Cursor indicator overlay - shows a small colored dot near the mouse pointer
to indicate recording/processing state.
"""

import logging
import threading

import gi

gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gtk, Gdk, GLib

logger = logging.getLogger(__name__)

INDICATOR_SIZE = 16
INDICATOR_OFFSET = 20


class CursorIndicator:
    """Small floating indicator that follows the cursor."""

    def __init__(self):
        self._window: Gtk.Window | None = None
        self._color = (0.2, 0.8, 0.2)  # Green default
        self._visible = False
        self._running = True
        self._update_thread: threading.Thread | None = None

    def _create_window(self) -> None:
        """Create the indicator window (must be called from GTK main thread)."""
        if self._window is not None:
            return

        self._window = Gtk.Window(type=Gtk.WindowType.POPUP)
        self._window.set_decorated(False)
        self._window.set_skip_taskbar_hint(True)
        self._window.set_skip_pager_hint(True)
        self._window.set_keep_above(True)
        self._window.set_accept_focus(False)
        self._window.set_default_size(INDICATOR_SIZE, INDICATOR_SIZE)
        self._window.set_size_request(INDICATOR_SIZE, INDICATOR_SIZE)

        # Enable transparency
        screen = self._window.get_screen()
        visual = screen.get_rgba_visual()
        if visual:
            self._window.set_visual(visual)
        self._window.set_app_paintable(True)

        # Drawing area for the indicator
        self._drawing_area = Gtk.DrawingArea()
        self._drawing_area.set_size_request(INDICATOR_SIZE, INDICATOR_SIZE)
        self._drawing_area.connect('draw', self._on_draw)
        self._window.add(self._drawing_area)

        self._window.show_all()
        self._window.hide()

    def _on_draw(self, widget, cr) -> bool:
        """Draw the indicator circle."""
        # Clear with transparency
        cr.set_source_rgba(0, 0, 0, 0)
        cr.set_operator(1)  # CAIRO_OPERATOR_SOURCE
        cr.paint()

        # Draw colored circle
        cr.set_operator(0)  # CAIRO_OPERATOR_CLEAR then OVER
        cr.set_operator(2)  # CAIRO_OPERATOR_OVER
        r, g, b = self._color
        cr.set_source_rgba(r, g, b, 0.9)
        cr.arc(INDICATOR_SIZE / 2, INDICATOR_SIZE / 2, INDICATOR_SIZE / 2 - 1, 0, 6.28)
        cr.fill()

        # Draw border
        cr.set_source_rgba(0, 0, 0, 0.5)
        cr.set_line_width(1)
        cr.arc(INDICATOR_SIZE / 2, INDICATOR_SIZE / 2, INDICATOR_SIZE / 2 - 1, 0, 6.28)
        cr.stroke()

        return True

    def _update_position(self) -> bool:
        """Update window position to follow cursor."""
        if not self._visible or self._window is None:
            return False

        try:
            display = Gdk.Display.get_default()
            if display:
                seat = display.get_default_seat()
                if seat:
                    pointer = seat.get_pointer()
                    if pointer:
                        _, x, y = pointer.get_position()
                        self._window.move(int(x) + INDICATOR_OFFSET, int(y) + INDICATOR_OFFSET)
        except Exception:
            pass

        return self._visible  # Continue if still visible

    def show(self, color: tuple[float, float, float]) -> None:
        """Show indicator with specified RGB color (0-1 range)."""
        self._color = color
        self._visible = True

        def _show():
            if self._window is None:
                self._create_window()
            self._drawing_area.queue_draw()
            self._update_position()
            self._window.show()
            # Start position updates
            GLib.timeout_add(50, self._update_position)

        GLib.idle_add(_show)

    def hide(self) -> None:
        """Hide the indicator."""
        self._visible = False

        def _hide():
            if self._window:
                self._window.hide()

        GLib.idle_add(_hide)

    def set_recording(self) -> None:
        """Show red indicator for recording."""
        self.show((0.9, 0.2, 0.2))

    def set_processing(self) -> None:
        """Show orange indicator for processing."""
        self.show((1.0, 0.6, 0.0))

    def set_idle(self) -> None:
        """Hide indicator when idle."""
        self.hide()

    def destroy(self) -> None:
        """Clean up resources."""
        self._running = False
        self._visible = False

        def _destroy():
            if self._window:
                self._window.destroy()
                self._window = None

        GLib.idle_add(_destroy)
