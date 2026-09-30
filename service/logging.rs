//! Bounded, asynchronous JSON-lines logging. Producers never perform disk I/O.
use crate::config::{expand, Config};
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const CAPACITY: usize = 2048;
const BACKUPS: usize = 5;
const MAX_MESSAGE: usize = 16 * 1024;
#[derive(Default)]
struct Health {
    dropped: AtomicU64,
    write_errors: AtomicU64,
}
static HEALTH: OnceLock<Arc<Health>> = OnceLock::new();
static SENDER: OnceLock<mpsc::SyncSender<Entry>> = OnceLock::new();
thread_local! { static CONTEXT: std::cell::RefCell<serde_json::Value> = const { std::cell::RefCell::new(serde_json::Value::Null) }; }
pub struct Context(serde_json::Value);
pub fn context(value: serde_json::Value) -> Context {
    Context(CONTEXT.with(|c| c.replace(value)))
}
pub fn current_context() -> serde_json::Value {
    CONTEXT.with(|c| c.borrow().clone())
}
impl Drop for Context {
    fn drop(&mut self) {
        CONTEXT.with(|c| {
            c.replace(self.0.take());
        });
    }
}
static SESSION: OnceLock<String> = OnceLock::new();
enum Entry {
    Record(String),
    Flush(mpsc::SyncSender<()>),
    Stop(mpsc::SyncSender<()>),
}
struct Logger {
    send: mpsc::SyncSender<Entry>,
    health: Arc<Health>,
    started: Instant,
    role: String,
}
pub fn session() -> &'static str {
    SESSION.get_or_init(|| {
        std::env::var("VOICE_DICTATION_SESSION").unwrap_or_else(|_| {
            format!(
                "{}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros(),
                std::process::id()
            )
        })
    })
}
fn clipped(value: &str) -> &str {
    let mut end = value.len().min(MAX_MESSAGE);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
            && (m.level() <= log::Level::Info || m.target().starts_with("voice_dictation"))
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let message = r.args().to_string();
        let thread = std::thread::current();
        let record = json!({"schema":1,"time_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "elapsed_ms":self.started.elapsed().as_millis(),"session":session(),"pid":std::process::id(),
            "thread":format!("{:?}",thread.id()),"thread_name":thread.name(),"role":self.role,
            "context":current_context(),"level":r.level().as_str(),"target":r.target(),"message":clipped(&message),"truncated":message.len()>MAX_MESSAGE});
        if self
            .send
            .try_send(Entry::Record(record.to_string()))
            .is_err()
        {
            self.health.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn flush(&self) {
        flush();
    }
}
struct Sink {
    path: PathBuf,
    file: Option<File>,
    bytes: u64,
    max: u64,
}
fn open(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}
fn backup(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}
impl Sink {
    fn new(path: PathBuf, max: u64) -> Self {
        Self {
            path,
            file: None,
            bytes: 0,
            max: max.max(1024),
        }
    }
    fn write(&mut self, line: &str) -> std::io::Result<()> {
        if self.file.is_none() {
            let file = open(&self.path)?;
            self.bytes = file.metadata()?.len();
            self.file = Some(file);
        }
        if self.bytes > 0 && self.bytes + line.len() as u64 + 1 > self.max {
            // Keep the active descriptor until rotation succeeds. Failed writes
            // retry opening on the next event and are visible in health counters.
            for i in (1..BACKUPS).rev() {
                match std::fs::rename(backup(&self.path, i), backup(&self.path, i + 1)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
            std::fs::rename(&self.path, backup(&self.path, 1))?;
            self.file = None;
            self.bytes = 0;
            self.file = Some(open(&self.path)?);
        }
        let file = self.file.as_mut().unwrap();
        if let Err(error) = writeln!(file, "{line}") {
            self.file = None;
            return Err(error);
        }
        self.bytes += line.len() as u64 + 1;
        Ok(())
    }
}
fn writer(receive: mpsc::Receiver<Entry>, mut sink: Sink, health: Arc<Health>) {
    let mut reported_drops = 0;
    let mut reported_errors = 0;
    while let Ok(entry) = receive.recv() {
        let dropped = health.dropped.load(Ordering::Relaxed);
        let errors = health.write_errors.load(Ordering::Relaxed);
        if dropped != reported_drops || errors != reported_errors {
            let report = json!({"schema":1,"time_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),"session":session(),"pid":std::process::id(),"level":"WARN","event":"logging_health","dropped_total":dropped,"write_errors_total":errors}).to_string();
            let _ = writeln!(std::io::stderr().lock(), "{report}");
            if sink.write(&report).is_ok() {
                reported_drops = dropped;
                reported_errors = errors;
            }
        }
        let stop = matches!(&entry, Entry::Stop(_));
        match entry {
            Entry::Record(line) => {
                // stderr belongs to this background thread, never the UI/coordinator.
                let _ = writeln!(std::io::stderr().lock(), "{line}");
                if let Err(error) = sink.write(&line) {
                    let count = health.write_errors.fetch_add(1, Ordering::Relaxed) + 1;
                    if count == 1 || count.is_power_of_two() {
                        let _ = writeln!(
                            std::io::stderr().lock(),
                            "{}",
                            json!({"event":"log_write_failed","path":sink.path,"error":error.to_string(),"failures":count})
                        );
                    }
                }
            }
            Entry::Flush(done) | Entry::Stop(done) => {
                if let Some(file) = &mut sink.file {
                    if file.sync_data().is_err() {
                        health.write_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let _ = done.send(());
                if stop {
                    break;
                }
            }
        }
    }
}
fn barrier(stop: bool) {
    if let Some(send) = SENDER.get() {
        let (done, receive) = mpsc::sync_channel(1);
        let mut entry = if stop {
            Entry::Stop(done)
        } else {
            Entry::Flush(done)
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match send.try_send(entry) {
                Ok(()) => {
                    let _ =
                        receive.recv_timeout(deadline.saturating_duration_since(Instant::now()));
                    break;
                }
                Err(mpsc::TrySendError::Full(returned)) if Instant::now() < deadline => {
                    entry = returned;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
    }
}
pub fn flush() {
    barrier(false);
}
pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        barrier(true);
    }
}
pub fn health() -> serde_json::Value {
    let health = HEALTH.get();
    json!({"session":session(),"queue_capacity":CAPACITY,"retained_backups":BACKUPS,
        "dropped_total":health.map_or(0, |h|h.dropped.load(Ordering::Relaxed)),
        "write_errors_total":health.map_or(0, |h|h.write_errors.load(Ordering::Relaxed))})
}
pub fn level(c: &Config) {
    log::set_max_level(match c.string("logging", "level") {
        "DEBUG" => log::LevelFilter::Debug,
        "WARNING" => log::LevelFilter::Warn,
        "ERROR" | "CRITICAL" => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    });
}
pub fn init(c: &Config) -> anyhow::Result<Guard> {
    init_role(c, "daemon")
}
pub fn init_role(c: &Config, role: &str) -> anyhow::Result<Guard> {
    let mut path = expand(c.string("logging", "file"));
    if role != "daemon" {
        path.set_file_name(format!("{role}.log"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let health = Arc::new(Health::default());
    let (send, receive) = mpsc::sync_channel(CAPACITY);
    // Separate component files avoid competing daemon/UI rotation. Multiple
    // windows use separate PID files for the same reason.
    if role != "daemon" {
        path.set_file_name(format!("{role}-{}.log", std::process::id()));
    }
    prune_components(&path);
    let sink = Sink::new(path, c.number("logging", "max_size_mb") * 1024 * 1024);
    std::thread::Builder::new()
        .name("log-writer".into())
        .spawn({
            let health = health.clone();
            move || writer(receive, sink, health)
        })?;
    SENDER
        .set(send.clone())
        .map_err(|_| anyhow::anyhow!("Logger already initialized"))?;
    let _ = HEALTH.set(health.clone());
    log::set_logger(Box::leak(Box::new(Logger {
        send,
        health,
        started: Instant::now(),
        role: role.into(),
    })))
    .map_err(|_| anyhow::anyhow!("Logger already initialized"))?;
    level(c);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(
            "Thread panic: {info}; backtrace={}",
            std::backtrace::Backtrace::force_capture()
        );
        flush();
        previous(info);
    }));
    let _startup = context(startup_record(c));
    log::info!(
        "Process started: version={} vulkan={} executable={:?}",
        env!("CARGO_PKG_VERSION"),
        cfg!(feature = "vulkan"),
        std::env::current_exe()
    );
    Ok(Guard)
}
fn startup_record(c: &Config) -> serde_json::Value {
    let executable = std::env::current_exe().ok();
    let receipt_path = executable
        .as_ref()
        .map(|p| p.with_file_name("receipt.json"));
    let receipt = receipt_path
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    json!({"event":"process_startup", "compiled_backend":if cfg!(feature="vulkan") {"vulkan"} else {"cpu"},
        "requested_profile":crate::optimization::profile(c), "model":c.string("transcription","model"),
        "config_path":c.path, "executable":executable, "receipt_path":receipt_path,
        "installation_identity":receipt.as_ref().map(|r| json!({"install_id":r["install_id"],
            "build_id":r["build_id"],"backend":r["backend"],"selection_source":r["selection_source"]})),
        "installation_evidence":if receipt.is_some() {"receipt_observed"} else {"historical_unknown"}})
}
fn prune_components(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    let mut completed = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((role, pid)) = name.strip_suffix(".log").and_then(|n| n.rsplit_once('-'))
            else {
                continue;
            };
            if !matches!(role, "desktop" | "hud" | "adapter" | "cli")
                || pid.parse::<u32>().is_err()
                || Path::new("/proc").join(pid).exists()
            {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                completed.push((meta.modified().ok(), entry.path()));
            }
        }
    }
    completed.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in completed.into_iter().skip(10) {
        let _ = std::fs::remove_file(&path);
        for i in 1..=BACKUPS {
            let _ = std::fs::remove_file(backup(&path, i));
        }
    }
}
pub fn recent(c: &Config) -> String {
    let path = expand(c.string("logging", "file"));
    let mut sections = Vec::new();
    for path in [backup(&path, 1), path] {
        if let Ok(mut file) = File::open(&path) {
            let len = file.metadata().map_or(0, |m| m.len());
            let _ = file.seek(SeekFrom::Start(len.saturating_sub(96000)));
            let mut bytes = Vec::new();
            let _ = file.take(96000).read_to_end(&mut bytes);
            sections.push(format!(
                "{}\n{}",
                path.display(),
                String::from_utf8_lossy(&bytes)
            ));
        }
    }
    sections.join("\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_records_backend_and_requested_profile_without_config_contents() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::at(dir.path())
            .unwrap()
            .changed(&json!({"performance":{"profile":"vulkan"},"private":"secret"}))
            .unwrap();
        let record = startup_record(&c);
        assert_eq!(record["event"], "process_startup");
        assert_eq!(record["requested_profile"], "vulkan");
        assert_eq!(record["config_path"], c.path.to_string_lossy().as_ref());
        assert!(!record.to_string().contains("secret"));
    }
    #[test]
    fn rotation_retains_five_backups_and_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let mut sink = Sink::new(path.clone(), 1024);
        for i in 0..8 {
            sink.write(&format!("{i} {}", "x".repeat(900))).unwrap();
        }
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("7 "));
        assert!(std::fs::read_to_string(backup(&path, 5))
            .unwrap()
            .starts_with("2 "));
        assert!(!backup(&path, 6).exists());
    }
    #[test]
    fn writer_failure_is_counted_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/app.log");
        let health = Arc::new(Health::default());
        let (tx, rx) = mpsc::sync_channel(8);
        let h = health.clone();
        let p = path.clone();
        let worker = std::thread::spawn(move || writer(rx, Sink::new(p, 1024), h));
        tx.send(Entry::Record("first".into())).unwrap();
        let (done, ack) = mpsc::sync_channel(1);
        tx.send(Entry::Flush(done)).unwrap();
        ack.recv().unwrap();
        assert_eq!(health.write_errors.load(Ordering::Relaxed), 1);
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        tx.send(Entry::Record("recovered".into())).unwrap();
        let (done, ack) = mpsc::sync_channel(1);
        tx.send(Entry::Stop(done)).unwrap();
        ack.recv().unwrap();
        worker.join().unwrap();
        assert!(std::fs::read_to_string(path).unwrap().contains("recovered"));
    }
    #[test]
    fn saturated_queue_does_not_block_and_counts_loss() {
        let (tx, _rx) = mpsc::sync_channel(1);
        let h = Arc::new(Health::default());
        let logger = Logger {
            send: tx,
            health: h.clone(),
            started: Instant::now(),
            role: "test".into(),
        };
        log::set_max_level(log::LevelFilter::Info);
        use log::Log;
        for _ in 0..10 {
            logger.log(
                &log::Record::builder()
                    .args(format_args!("test"))
                    .level(log::Level::Info)
                    .build(),
            );
        }
        assert_eq!(h.dropped.load(Ordering::Relaxed), 9);
    }
}
