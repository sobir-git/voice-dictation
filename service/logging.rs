use crate::config::{expand, Config};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    sync::Mutex,
};
struct Logger {
    file: Mutex<File>,
    path: std::path::PathBuf,
    max: u64,
}
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let message = format!(
            "{} {} {}: {}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            r.level(),
            r.target(),
            r.args()
        );
        eprint!("{message}");
        if let Ok(mut file) = self.file.lock() {
            if file.metadata().is_ok_and(|m| m.len() > self.max) {
                let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
                if let Ok(next) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                {
                    *file = next;
                }
            }
            let _ = file.write_all(message.as_bytes());
        }
    }
    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}
pub fn level(c: &Config) {
    log::set_max_level(match c.string("logging", "level") {
        "DEBUG" => log::LevelFilter::Debug,
        "WARNING" => log::LevelFilter::Warn,
        "ERROR" | "CRITICAL" => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    });
}
pub fn init(c: &Config) -> anyhow::Result<()> {
    let path = expand(c.string("logging", "file"));
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let logger = Box::new(Logger {
        file: Mutex::new(file),
        path,
        max: c.number("logging", "max_size_mb") * 1024 * 1024,
    });
    let _ = log::set_logger(Box::leak(logger));
    level(c);
    Ok(())
}
pub fn recent(c: &Config) -> String {
    let path = expand(c.string("logging", "file"));
    let mut bytes = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(64000)));
        let _ = file.read_to_end(&mut bytes);
    }
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .rev()
        .take(120)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}
