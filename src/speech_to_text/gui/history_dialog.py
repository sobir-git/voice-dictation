from speech_to_text.gui.gtk import Gtk
from speech_to_text.gui.history_manager import HistoryManager


class HistoryDialog(Gtk.Dialog):
    def __init__(self, parent, history: HistoryManager):
        super().__init__(title='Transcription History', transient_for=parent, flags=0)
        self.history = history
        self.set_default_size(600, 400)
        self.add_buttons(Gtk.STOCK_CLOSE, Gtk.ResponseType.CLOSE)

        self.store = Gtk.ListStore(int, str, str)
        self.tree = Gtk.TreeView(model=self.store)
        renderer = Gtk.CellRendererText()
        time_column = Gtk.TreeViewColumn('Time', renderer, text=1)
        text_column = Gtk.TreeViewColumn('Transcription', renderer, text=2)
        time_column.set_min_width(140)
        text_column.set_expand(True)
        self.tree.append_column(time_column)
        self.tree.append_column(text_column)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        scroll.add(self.tree)

        box = self.get_content_area()
        box.pack_start(scroll, True, True, 0)
        button = Gtk.Button(label='Copy to Clipboard')
        button.connect('clicked', self._on_copy)
        box.pack_start(button, False, False, 8)

        self._load()
        self.show_all()

    def _load(self) -> None:
        self.store.clear()
        for item in self.history.get_recent(50):
            text = item['text'][:80] + '...' if len(item['text']) > 80 else item['text']
            self.store.append([item['id'], item['timestamp'], text])

    def _on_copy(self, _widget) -> None:
        model, item_iter = self.tree.get_selection().get_selected()
        if item_iter is None:
            return
        item = self.history.get_by_id(model[item_iter][0])
        if not item:
            return
        clipboard = Gtk.Clipboard.get_default(self.get_display())
        clipboard.set_text(item['text'], -1)
        clipboard.store()
