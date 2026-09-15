use crate::{config::Config, process};
use ksni::blocking::TrayMethods;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

pub fn desktop() {
    thread::spawn(|| {
        if let Ok(mut child) = Command::new(project().join("run.sh"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let _ = child.wait();
        }
    });
}
pub fn project() -> std::path::PathBuf {
    std::env::var_os("VOICE_DICTATION_PROJECT")
        .map(Into::into)
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())
}
struct Tray {
    state: Value,
    command: mpsc::SyncSender<Value>,
}
impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "voice-dictation".into()
    }
    fn title(&self) -> String {
        format!("Voice Dictation: {}", status(&self.state))
    }
    fn icon_name(&self) -> String {
        icon_name(&self.state).into()
    }
    fn icon_theme_path(&self) -> String {
        project().join("icons/proposal").display().to_string()
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        desktop()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Voice Dictation".into(),
            description: status(&self.state).into(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open Voice Dictation".into(),
                activate: Box::new(|_| desktop()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if self.state["listening"] == true {
                    "Pause dictation"
                } else {
                    "Resume dictation"
                }
                .into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command.try_send(json!({"cmd":"toggle_listening"}));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Cancel dictation".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command.try_send(json!({"cmd":"cancel"}));
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit Voice Dictation".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command.try_send(json!({"cmd":"quit"}));
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
fn status(s: &Value) -> &str {
    if s["recording"] == true {
        "Recording"
    } else if s["processing"] == true {
        "Transcribing"
    } else if s["listening"] == true {
        "Ready"
    } else {
        "Paused"
    }
}
fn icon_name(s: &Value) -> &'static str {
    if s["recording"] == true {
        "voice-dictation-recording"
    } else if s["processing"] == true {
        "voice-dictation-transcribing"
    } else if s["last_error"]
        .as_str()
        .is_some_and(|error| !error.is_empty())
    {
        "voice-dictation-error"
    } else if s["listening"] == true {
        "voice-dictation-ready"
    } else {
        "voice-dictation-paused"
    }
}
#[derive(Default)]
struct Hud {
    child: Option<Child>,
    failures: u32,
    retry_at: Option<Instant>,
    pending: Vec<u8>,
    acknowledgement: Vec<u8>,
    sequence: u64,
    mode: Option<bool>,
    awaiting: Option<(u64, Instant)>,
    last_sent: Option<Instant>,
}
impl Hud {
    fn update(&mut self, state: &Value, enabled: bool) {
        if !enabled || !(state["recording"] == true || state["processing"] == true) {
            self.close();
            self.failures = 0;
            self.retry_at = None;
            return;
        }
        if self.child.is_none() {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return;
            }
            if std::env::var_os("DISPLAY").is_none() {
                return;
            }
            let position = process::run(
                Command::new("xdotool").args(["getmouselocation", "--shell", "getdisplaygeometry"]),
                None,
                Duration::from_secs(1),
            )
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| {
                let value = |key: &str| {
                    s.lines()
                        .find_map(|line| line.strip_prefix(key).and_then(|s| s.parse::<i32>().ok()))
                        .unwrap_or(40)
                };
                let geometry = s
                    .lines()
                    .last()
                    .unwrap_or("")
                    .split_whitespace()
                    .filter_map(|v| v.parse::<i32>().ok())
                    .collect::<Vec<_>>();
                let mut x = value("X=").saturating_add(20);
                let mut y = value("Y=").saturating_add(24);
                if let [width, height] = geometry.as_slice() {
                    x = x.clamp(0, (width - 168).max(0));
                    y = y.clamp(0, (height - 60).max(0));
                }
                format!("{x},{y}")
            })
            .unwrap_or_else(|| "60,60".into());
            let executable = std::env::current_exe()
                .unwrap_or_else(|_| project().join("target/release/speech-service"))
                .with_file_name("voice-dictation");
            let mut command = Command::new(&executable);
            command.env("VOICE_DICTATION_SESSION", crate::logging::session());
            process::kill_with_parent(&mut command);
            let child = command
                .arg("--hud")
                .env_remove("WAYLAND_DISPLAY")
                .env("WINIT_UNIX_BACKEND", "x11")
                .env("VOICE_DICTATION_HUD_POSITION", position)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn();
            match child {
                Ok(mut child) => {
                    if let Some(stdin) = &child.stdin {
                        if let Err(error) = process::nonblocking(stdin) {
                            log::warn!("Floating indicator pipe: {error}");
                            let _ = child.kill();
                            let _ = child.wait();
                            self.fail();
                            return;
                        }
                    }
                    if let Some(stdout) = &child.stdout {
                        if let Err(error) = process::nonblocking(stdout) {
                            log::warn!("Floating indicator acknowledgement pipe: {error}");
                            let _ = child.kill();
                            let _ = child.wait();
                            self.fail();
                            return;
                        }
                    }
                    log::info!("Floating indicator started: pid={}", child.id());
                    self.child = Some(child)
                }
                Err(e) => {
                    log::warn!("Floating indicator {}: {e}", executable.display());
                    self.fail();
                }
            }
        }
        self.check_acknowledgement();
        let mode = state["recording"] == true;
        if self.child.is_some()
            && (self.mode != Some(mode)
                || (self.awaiting.is_none()
                    && self
                        .last_sent
                        .is_none_or(|at| at.elapsed() > Duration::from_secs(1))))
        {
            self.mode = Some(mode);
            self.sequence += 1;
            self.awaiting = Some((self.sequence, Instant::now()));
            self.last_sent = Some(Instant::now());
            log::info!(
                "Floating indicator state requested: sequence={} recording={mode} jobs={}",
                self.sequence,
                state["jobs"]
            );
        }
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!(
                        "Floating indicator exited: pid={} status={status}",
                        child.id()
                    );
                    self.fail();
                    return;
                }
                Err(error) => {
                    log::warn!("Floating indicator status failed: {error}");
                    self.fail();
                    return;
                }
                Ok(None) => {}
            }
            if let Some(stdin) = &mut child.stdin {
                // Preserve a partially written JSON line. Append the newest frame
                // only after it drains, so the reader always receives valid lines.
                if self.pending.is_empty() {
                    self.pending = json!({"recording":state["recording"],"level":state["level"],"sequence":self.sequence,"daemon_session":crate::logging::session()})
                        .to_string()
                        .into_bytes();
                    self.pending.push(b'\n');
                }
                while !self.pending.is_empty() {
                    match stdin.write(&self.pending) {
                        Ok(0) => {
                            log::warn!("Floating indicator pipe closed");
                            self.fail();
                            return;
                        }
                        Ok(n) => {
                            self.pending.drain(..n);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => {
                            log::warn!("Floating indicator write failed: {error}");
                            self.fail();
                            return;
                        }
                    }
                }
            }
        }
    }
    fn check_acknowledgement(&mut self) {
        let Some(child) = &mut self.child else {
            return;
        };
        if let Some(stdout) = &mut child.stdout {
            let mut block = [0; 1024];
            loop {
                match stdout.read(&mut block) {
                    Ok(0) => break,
                    Ok(n) => self.acknowledgement.extend_from_slice(&block[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        log::warn!("Floating indicator acknowledgement read: {error}");
                        break;
                    }
                }
                if self.acknowledgement.len() > 4096 {
                    self.acknowledgement.clear();
                    log::warn!("Floating indicator acknowledgement overflow");
                    break;
                }
            }
        }
        while let Some(end) = self.acknowledgement.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.acknowledgement.drain(..=end).collect();
            if let Ok(sequence) = String::from_utf8_lossy(&line).trim().parse::<u64>() {
                if let Some((expected, started)) = self.awaiting {
                    if sequence == expected {
                        log::info!("Floating indicator paint acknowledged: worker_pid={} sequence={sequence} elapsed_ms={}",child.id(),started.elapsed().as_millis());
                        self.awaiting = None;
                    }
                }
            }
        }
        if self
            .awaiting
            .is_some_and(|(_, started)| started.elapsed() > Duration::from_secs(3))
        {
            log::error!(
                "Floating indicator paint timeout: worker_pid={} sequence={} timeout_ms=3000",
                child.id(),
                self.sequence
            );
            self.fail();
        }
    }
    fn fail(&mut self) {
        self.close();
        self.failures = self.failures.saturating_add(1);
        let delay = Duration::from_secs(1 << self.failures.min(6)).min(Duration::from_secs(60));
        self.retry_at = Some(Instant::now() + delay);
    }
    fn close(&mut self) {
        self.pending.clear();
        self.acknowledgement.clear();
        self.mode = None;
        self.awaiting = None;
        if let Some(mut child) = self.child.take() {
            log::info!("Floating indicator closing: pid={}", child.id());
            let _ = child.kill();
            match child.wait() {
                Ok(status) => log::info!(
                    "Floating indicator reaped: pid={} status={status}",
                    child.id()
                ),
                Err(error) => log::warn!("Floating indicator reap failed: {error}"),
            };
        }
    }
}
impl Drop for Hud {
    fn drop(&mut self) {
        self.close()
    }
}
// A single replaceable snapshot prevents level traffic from dropping a final
// idle/error transition. The producer never waits for the tray or HUD to render.
#[derive(Default)]
struct MailboxSlot {
    latest: Option<(Value, bool)>,
    closed: bool,
}
enum MailboxEvent {
    State(Value, bool),
    Tick,
    Closed,
}
#[derive(Default)]
struct StateMailbox {
    state: Mutex<MailboxSlot>,
    wake: Condvar,
}
impl StateMailbox {
    fn publish(&self, state: Value, enabled: bool) {
        let mut slot = self.state.lock().unwrap();
        slot.latest = Some((state, enabled));
        self.wake.notify_one();
    }
    fn receive(&self, active: bool) -> MailboxEvent {
        let mut slot = self.state.lock().unwrap();
        while slot.latest.is_none() && !slot.closed {
            if active {
                let (next, timeout) = self
                    .wake
                    .wait_timeout(slot, Duration::from_millis(250))
                    .unwrap();
                slot = next;
                if timeout.timed_out() && slot.latest.is_none() && !slot.closed {
                    return MailboxEvent::Tick;
                }
            } else {
                slot = self.wake.wait(slot).unwrap();
            }
        }
        match slot.latest.take() {
            Some((state, enabled)) => MailboxEvent::State(state, enabled),
            None => MailboxEvent::Closed,
        }
    }
    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.wake.notify_one();
    }
}
pub struct Companion {
    mailbox: Arc<StateMailbox>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Companion {
    pub fn start(command: mpsc::SyncSender<Value>) -> Self {
        let mailbox = Arc::new(StateMailbox::default());
        let receive = mailbox.clone();
        let thread = thread::spawn(move || {
            let tray = Tray {
                state: json!({}),
                command,
            }
            .assume_sni_available(true)
            .spawn()
            .map_err(|e| log::warn!("Tray unavailable: {e}"))
            .ok();
            let mut hud = Hud::default();
            let mut previous = Value::Null;
            let mut latest = (Value::Null, false);
            loop {
                match receive.receive(hud.child.is_some() || hud.retry_at.is_some()) {
                    MailboxEvent::Closed => break,
                    MailboxEvent::State(state, enabled) => latest = (state, enabled),
                    MailboxEvent::Tick => {}
                }
                let (state, enabled) = (&latest.0, latest.1);
                let flags = json!({"recording":state["recording"],"processing":state["processing"],"listening":state["listening"],"last_error":state["last_error"]});
                if flags != previous {
                    if let Some(handle) = &tray {
                        handle.update(|t| t.state = flags.clone());
                    }
                    previous = flags;
                }
                hud.update(state, enabled);
            }
            if let Some(tray) = tray {
                tray.shutdown().wait();
            }
        });
        Self {
            mailbox,
            thread: Some(thread),
        }
    }
    pub fn update(&self, state: Value, c: &Config) {
        self.mailbox
            .publish(state, c.flag("ui", "cursor_indicator"));
    }
}
impl Drop for Companion {
    fn drop(&mut self) {
        self.mailbox.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{icon_name, MailboxEvent, StateMailbox};
    use serde_json::json;

    #[test]
    fn missing_paint_acknowledgement_reaps_the_child() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        crate::process::nonblocking(child.stdout.as_ref().unwrap()).unwrap();
        let mut hud = super::Hud::default();
        hud.child = Some(child);
        hud.awaiting = Some((
                1,
                std::time::Instant::now() - std::time::Duration::from_secs(4),
            ));
        hud.check_acknowledgement();
        assert!(hud.child.is_none());
        assert!(hud.retry_at.is_some());
    }
    #[test]
    fn matching_paint_acknowledgement_clears_deadline() {
        let mut child = std::process::Command::new("printf")
            .arg("7\n")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let mut hud = super::Hud::default();
        hud.child = Some(child);
        hud.awaiting = Some((7, std::time::Instant::now()));
        hud.check_acknowledgement();
        assert!(hud.awaiting.is_none());
    }
    #[test]
    fn final_idle_state_survives_a_slow_companion() {
        let mailbox = StateMailbox::default();
        for _ in 0..1000 {
            mailbox.publish(json!({"recording":true,"level":0.5}), true);
        }
        mailbox.publish(json!({"recording":false,"processing":false}), true);
        mailbox.close();
        let MailboxEvent::State(state, enabled) = mailbox.receive(false) else {
            panic!("missing state")
        };
        assert!(enabled);
        assert_eq!(state["recording"], false);
        assert_eq!(state["processing"], false);
        assert!(matches!(mailbox.receive(false), MailboxEvent::Closed));
    }

    #[test]
    fn tray_icons_follow_state_priority() {
        assert_eq!(
            icon_name(&json!({"listening":true})),
            "voice-dictation-ready"
        );
        assert_eq!(
            icon_name(&json!({"listening":false})),
            "voice-dictation-paused"
        );
        assert_eq!(
            icon_name(&json!({"listening":true,"last_error":"Microphone unavailable"})),
            "voice-dictation-error"
        );
        assert_eq!(
            icon_name(&json!({"listening":true,"processing":true,"last_error":"Earlier error"})),
            "voice-dictation-transcribing"
        );
        assert_eq!(
            icon_name(&json!({"listening":true,"recording":true,"processing":true})),
            "voice-dictation-recording"
        );
    }
}
