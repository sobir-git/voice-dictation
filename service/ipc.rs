use anyhow::{bail, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};
pub fn socket_path() -> PathBuf {
    std::env::var_os("STT_SOCKET_PATH")
        .map(Into::into)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    format!("/tmp/speech-to-text-{}", unsafe { libc::getuid() }).into()
                })
                .join("speech-to-text/daemon.sock")
        })
}
pub fn stopped_path() -> PathBuf {
    socket_path().with_extension("stopped")
}
pub struct Server {
    pub receive: mpsc::Receiver<crate::daemon::Event>,
    pub send: mpsc::SyncSender<crate::daemon::Event>,
    stop: Arc<AtomicBool>,
    _lock: File,
    path: PathBuf,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn bind(path: PathBuf) -> Result<Self> {
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path.with_extension("sock.lock"))?;
        if lock.try_lock_exclusive().is_err() {
            bail!("Speech service is already running")
        }
        if path.exists() {
            std::fs::remove_file(&path)?
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (send, receive) = mpsc::sync_channel(256);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let events = send.clone();
        let thread = thread::spawn(move || {
            let mut id = 0;
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        id += 1;
                        let events = events.clone();
                        let flag = flag.clone();
                        let id = id;
                        thread::spawn(move || {
                            let _ = serve_client(id, stream, events, flag);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            receive,
            send,
            stop,
            _lock: lock,
            path,
            thread: Some(thread),
        })
    }
}
fn serve_client(
    id: u64,
    stream: UnixStream,
    events: mpsc::SyncSender<crate::daemon::Event>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    use crate::daemon::Event;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let mut writer = stream.try_clone()?;
    let (send, receive) = mpsc::sync_channel::<Value>(64);
    events.send(Event::Connect(id, send))?;
    let thread = thread::spawn(move || {
        while let Ok(value) = receive.recv() {
            if writeln!(writer, "{value}").is_err() {
                break;
            }
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });
    // Keep partial records across socket timeouts, and bound client input.
    let mut pending = Vec::new();
    let mut reader = stream;
    let mut block = [0; 4096];
    while !stop.load(Ordering::Relaxed) {
        match reader.read(&mut block) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&block[..n]);
                if pending.len() > 65536 {
                    break;
                }
                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                    let line: Vec<_> = pending.drain(..=end).collect();
                    match serde_json::from_slice::<Value>(&line) {
                        Ok(value) if value.is_object() => {
                            if events.send(Event::Command(id, value)).is_err() {
                                break;
                            }
                        }
                        _ => {
                            let _ = events.send(Event::Command(id, json!({"cmd":"invalid"})));
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = events.send(Event::Disconnect(id));
    let _ = reader.shutdown(std::net::Shutdown::Both);
    let _ = thread.join();
    Ok(())
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
pub fn ready() -> bool {
    UnixStream::connect(socket_path())
        .and_then(|stream| {
            stream.set_read_timeout(Some(Duration::from_millis(500)))?;
            let mut line = String::new();
            BufReader::new(stream).take(65536).read_line(&mut line)?;
            Ok(serde_json::from_str::<Value>(&line).is_ok_and(|v| v["protocol"] == 2))
        })
        .unwrap_or(false)
}
pub fn ensure_daemon() -> Result<()> {
    if ready() {
        return Ok(());
    }
    if std::env::var_os("STT_SOCKET_PATH").is_none()
        && crate::config::home()
            .join(".config/systemd/user/speech-to-text-daemon.service")
            .exists()
    {
        crate::process::run(
            std::process::Command::new("systemctl").args([
                "--user",
                "start",
                "--no-block",
                "speech-to-text-daemon.service",
            ]),
            None,
            Duration::from_secs(5),
        )?;
    } else {
        use std::os::unix::process::CommandExt;
        let binary = std::env::current_exe()?.with_file_name("speech-service");
        let mut command = std::process::Command::new(binary);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // SAFETY: setsid is called between fork and exec without allocating or locking.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
    for _ in 0..100 {
        if ready() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    bail!("Rust speech service did not become ready; check service logs")
}
