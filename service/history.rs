use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;

pub struct History(Connection);
impl History {
    pub fn open(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let db = Connection::open(directory.join("history.db"))?;
        db.busy_timeout(std::time::Duration::from_secs(3))?;
        db.execute("CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY AUTOINCREMENT, timestamp TEXT, text TEXT)", [])?;
        let columns = db
            .prepare("PRAGMA table_info(history)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        for (name, definition) in [
            ("audio_path", "TEXT"),
            ("duration", "REAL NOT NULL DEFAULT 0"),
            ("favorite", "INTEGER NOT NULL DEFAULT 0"),
            ("failed", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !columns.iter().any(|column| column == name) {
                db.execute(
                    &format!("ALTER TABLE history ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }
        Ok(Self(db))
    }
    pub fn add(&self, text: &str) -> Result<i64> {
        self.0.execute("INSERT INTO history(timestamp,text) VALUES (strftime('%Y-%m-%d %H:%M:%S','now','localtime'),?)",[text])?;
        Ok(self.0.last_insert_rowid())
    }
    pub fn recent(&self, limit: usize, search: &str) -> Result<Vec<Value>> {
        let mut query=self.0.prepare("SELECT id,timestamp,text,audio_path,duration,favorite,failed FROM history WHERE instr(lower(text),lower(?)) > 0 ORDER BY favorite DESC,id DESC LIMIT ?")?;
        let rows=query.query_map(params![search,limit.min(1000) as i64],|r| Ok(json!({"id":r.get::<_,i64>(0)?,"timestamp":r.get::<_,String>(1)?,"text":r.get::<_,String>(2)?,"audio_path":r.get::<_,Option<String>>(3)?,"duration":r.get::<_,f64>(4)?,"favorite":r.get::<_,bool>(5)?,"failed":r.get::<_,bool>(6)?})))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn add_recording(
        &self,
        text: &str,
        audio_path: &Path,
        duration: f64,
        failed: bool,
    ) -> Result<i64> {
        self.0.execute(
            "INSERT INTO history(timestamp,text,audio_path,duration,failed) VALUES (strftime('%Y-%m-%d %H:%M:%S','now','localtime'),?,?,?,?)",
            params![text, audio_path.to_string_lossy(), duration, failed],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        self.0.execute(
            "UPDATE history SET favorite=? WHERE id=?",
            params![favorite, id],
        )?;
        Ok(())
    }

    pub fn update_transcription(&self, id: i64, text: &str, failed: bool) -> Result<()> {
        self.0.execute(
            "UPDATE history SET text=?,failed=? WHERE id=?",
            params![text, failed, id],
        )?;
        Ok(())
    }

    pub fn remove(&self, id: i64) -> Result<Option<String>> {
        let path = self
            .0
            .query_row("SELECT audio_path FROM history WHERE id=?", [id], |row| {
                row.get(0)
            })
            .ok()
            .flatten();
        self.0.execute("DELETE FROM history WHERE id=?", [id])?;
        Ok(path)
    }

    pub fn get(&self, id: i64) -> Result<Option<Value>> {
        let mut query = self.0.prepare(
            "SELECT id,timestamp,text,audio_path,duration,favorite,failed FROM history WHERE id=?",
        )?;
        let mut rows = query.query([id])?;
        Ok(rows.next()?.map(|r| json!({"id":r.get::<_,i64>(0).unwrap(),"timestamp":r.get::<_,String>(1).unwrap(),"text":r.get::<_,String>(2).unwrap(),"audio_path":r.get::<_,Option<String>>(3).unwrap(),"duration":r.get::<_,f64>(4).unwrap(),"favorite":r.get::<_,bool>(5).unwrap(),"failed":r.get::<_,bool>(6).unwrap()})))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_python_schema_and_preserves_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join("history.db")).unwrap();
        db.execute_batch("CREATE TABLE history (id INTEGER PRIMARY KEY AUTOINCREMENT,timestamp TEXT,text TEXT); INSERT INTO history VALUES (7,'2026-01-01 01:02:03','old entry');").unwrap();
        drop(db);
        let h = History::open(dir.path()).unwrap();
        assert_eq!(h.add("Салом Rust").unwrap(), 8);
        assert_eq!(h.recent(100, "OLD").unwrap()[0]["id"], 7);
        assert_eq!(h.recent(100, "Rust").unwrap()[0]["text"], "Салом Rust");
    }

    #[test]
    fn migrates_and_manages_recording_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path()).unwrap();
        let audio = dir.path().join("recording.wav");
        std::fs::write(&audio, b"synthetic").unwrap();
        let id = history.add_recording("", &audio, 3.4, true).unwrap();
        history
            .update_transcription(id, "recovered words", false)
            .unwrap();
        history.set_favorite(id, true).unwrap();
        let item = history.get(id).unwrap().unwrap();
        assert_eq!(item["text"], "recovered words");
        assert_eq!(item["favorite"], true);
        assert_eq!(item["failed"], false);
        assert_eq!(
            history.remove(id).unwrap(),
            Some(audio.to_string_lossy().into_owned())
        );
        assert!(history.get(id).unwrap().is_none());
    }
}
