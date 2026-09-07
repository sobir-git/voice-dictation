import os
import sqlite3
import time
from pathlib import Path


class HistoryManager:
    def __init__(self):
        self.data_dir = Path(os.path.expanduser('~/.local/share/speech-to-text'))
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.db_path = self.data_dir / 'history.db'
        self._init_db()

    def _init_db(self) -> None:
        with sqlite3.connect(self.db_path) as conn:
            conn.execute(
                'CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY AUTOINCREMENT, timestamp TEXT, text TEXT)'
            )
            conn.commit()

    def add(self, text: str) -> int:
        ts = time.strftime('%Y-%m-%d %H:%M:%S')
        with sqlite3.connect(self.db_path) as conn:
            cur = conn.execute('INSERT INTO history(timestamp, text) VALUES (?, ?)', (ts, text))
            conn.commit()
            return int(cur.lastrowid)

    def get_recent(self, limit: int = 100, search: str = ""):
        with sqlite3.connect(self.db_path) as conn:
            cur = conn.execute(
                'SELECT id, timestamp, text FROM history WHERE instr(lower(text), lower(?)) > 0 ORDER BY id DESC LIMIT ?',
                (search, limit))
            return [{'id': r[0], 'timestamp': r[1], 'text': r[2]} for r in cur.fetchall()]

    def get_by_id(self, item_id: int):
        with sqlite3.connect(self.db_path) as conn:
            cur = conn.execute('SELECT id, timestamp, text FROM history WHERE id = ?', (item_id,))
            row = cur.fetchone()
            if not row:
                return None
            return {'id': row[0], 'timestamp': row[1], 'text': row[2]}
