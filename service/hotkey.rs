use evdev::{Device, EventType};
use std::{
    collections::{HashMap, HashSet},
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Debug)]
pub enum KeyEvent {
    Down,
    Up,
    Captured(String),
    Missing,
}
pub struct Hotkeys {
    pub key: Arc<AtomicU16>,
    pub capture: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Hotkeys {
    pub fn start(code: u16, emit: impl Fn(KeyEvent) + Send + 'static) -> Self {
        let key = Arc::new(AtomicU16::new(code));
        let capture = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (k, c, s) = (key.clone(), capture.clone(), stop.clone());
        let thread = thread::spawn(move || {
            let mut devices: HashMap<PathBuf, Device> = HashMap::new();
            let mut held = HashSet::new();
            let mut scan = Instant::now() - Duration::from_secs(3);
            let mut previous = code;
            let mut missing = false;
            while !s.load(Ordering::Relaxed) {
                let code = k.load(Ordering::Relaxed);
                if code != previous {
                    if !held.is_empty() {
                        held.clear();
                        emit(KeyEvent::Up);
                    }
                    previous = code;
                }
                if scan.elapsed() >= Duration::from_secs(2) {
                    scan = Instant::now();
                    for (path, device) in evdev::enumerate() {
                        if devices.contains_key(&path) || device.supported_keys().is_none() {
                            continue;
                        }
                        let name = device.name().unwrap_or("").to_lowercase();
                        if name.contains("ydotool") || name.contains("dotool") {
                            continue;
                        }
                        if device.set_nonblocking(true).is_ok() {
                            devices.insert(path, device);
                        }
                    }
                    let available = devices.values().any(|d| {
                        d.supported_keys()
                            .is_some_and(|keys| keys.contains(evdev::KeyCode::new(code)))
                    });
                    if !available && !missing {
                        emit(KeyEvent::Missing)
                    }
                    missing = !available;
                }
                let paths: Vec<_> = devices.keys().cloned().collect();
                let mut fds: Vec<_> = paths
                    .iter()
                    .map(|p| libc::pollfd {
                        fd: devices[p].as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    })
                    .collect();
                // SAFETY: every fd belongs to a live device; the vector remains allocated for poll.
                unsafe {
                    libc::poll(fds.as_mut_ptr(), fds.len() as _, 200);
                }
                for (path, fd) in paths.iter().zip(&fds) {
                    if fd.revents == 0 {
                        continue;
                    }
                    let events = devices
                        .get_mut(path)
                        .unwrap()
                        .fetch_events()
                        .map(|events| events.collect::<Vec<_>>());
                    match events {
                        Ok(events) => {
                            for event in events {
                                if event.event_type() != EventType::KEY {
                                    continue;
                                }
                                if c.load(Ordering::Relaxed) {
                                    if event.value() == 1 {
                                        emit(KeyEvent::Captured(format!(
                                            "{:?}",
                                            evdev::KeyCode::new(event.code())
                                        )));
                                    }
                                    continue;
                                }
                                if event.code() != code {
                                    continue;
                                }
                                if event.value() == 1 {
                                    let empty = held.is_empty();
                                    held.insert(path.clone());
                                    if empty {
                                        emit(KeyEvent::Down)
                                    }
                                } else if event.value() == 0 && held.remove(path) && held.is_empty()
                                {
                                    emit(KeyEvent::Up)
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => {
                            devices.remove(path);
                            if held.remove(path) && held.is_empty() {
                                emit(KeyEvent::Up)
                            }
                        }
                    }
                }
            }
            if !held.is_empty() {
                emit(KeyEvent::Up)
            }
        });
        Self {
            key,
            capture,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
