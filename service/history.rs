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
        Ok(Self(db))
    }
    pub fn add(&self, text: &str) -> Result<i64> {
        self.0.execute("INSERT INTO history(timestamp,text) VALUES (strftime('%Y-%m-%d %H:%M:%S','now','localtime'),?)",[text])?;
        Ok(self.0.last_insert_rowid())
    }
    pub fn recent(&self, limit: usize, search: &str) -> Result<Vec<Value>> {
        let mut query=self.0.prepare("SELECT id,timestamp,text FROM history WHERE instr(lower(text),lower(?)) > 0 ORDER BY id DESC LIMIT ?")?;
        let rows=query.query_map(params![search,limit.min(1000) as i64],|r| Ok(json!({"id":r.get::<_,i64>(0)?,"timestamp":r.get::<_,String>(1)?,"text":r.get::<_,String>(2)?})))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
}
