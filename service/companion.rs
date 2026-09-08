use crate::{config::Config, process};
use ksni::blocking::TrayMethods;
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
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
        if self.state["recording"] == true {
            "media-record"
        } else if self.state["processing"] == true {
            "view-refresh"
        } else if self.state["listening"] == true {
            "audio-input-microphone"
        } else {
            "microphone-sensitivity-muted"
        }
        .into()
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
#[derive(Default)]
struct Hud {
    child: Option<Child>,
    failed: bool,
}
impl Hud {
    fn update(&mut self, state: &Value, enabled: bool) {
        if !enabled || !(state["recording"] == true || state["processing"] == true) {
            self.close();
            self.failed = false;
            return;
        }
        if self.failed {
            return;
        }
        if self.child.is_none() {
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
                    x = x.clamp(0, (width - 280).max(0));
                    y = y.clamp(0, (height - 58).max(0));
                }
                format!("{x},{y}")
            })
            .unwrap_or_else(|| "60,60".into());
            let child = Command::new(
                std::env::current_exe()
                    .unwrap_or_else(|_| project().join("target/release/speech-service"))
                    .with_file_name("voice-dictation"),
            )
            .arg("--hud")
            .env_remove("WAYLAND_DISPLAY")
            .env("WINIT_UNIX_BACKEND", "x11")
            .env("VOICE_DICTATION_HUD_POSITION", position)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn();
            match child {
                Ok(mut child) => {
                    if let Some(stdin) = &child.stdin {
                        if let Err(error) = process::nonblocking(stdin) {
                            log::warn!("Floating indicator pipe: {error}");
                            self.failed = true;
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        }
                    }
                    self.child = Some(child)
                }
                Err(e) => {
                    self.failed = true;
                    log::warn!("Floating indicator: {e}");
                }
            }
        }
        if let Some(child) = &mut self.child {
            if child.try_wait().ok().flatten().is_some() {
                self.close();
                self.failed = true;
                return;
            }
            if let Some(stdin) = &mut child.stdin {
                let payload = json!({"recording":state["recording"],"level":state["level"]})
                    .to_string()
                    + "\n";
                if let Err(e) = stdin.write_all(payload.as_bytes()) {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        self.close();
                        self.failed = true;
                    }
                }
            }
        }
    }
    fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for Hud {
    fn drop(&mut self) {
        self.close()
    }
}
pub struct Companion {
    send: Option<mpsc::SyncSender<(Value, bool)>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Companion {
    pub fn start(command: mpsc::SyncSender<Value>) -> Self {
        let (send, receive) = mpsc::sync_channel::<(Value, bool)>(32);
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
            while let Ok((state, enabled)) = receive.recv() {
                let flags = json!({"recording":state["recording"],"processing":state["processing"],"listening":state["listening"]});
                if flags != previous {
                    if let Some(handle) = &tray {
                        handle.update(|t| t.state = flags.clone());
                    }
                    previous = flags;
                }
                hud.update(&state, enabled);
            }
            if let Some(tray) = tray {
                tray.shutdown().wait();
            }
        });
        Self {
            send: Some(send),
            thread: Some(thread),
        }
    }
    pub fn update(&self, state: Value, c: &Config) {
        if let Some(send) = &self.send {
            let _ = send.try_send((state, c.flag("ui", "cursor_indicator")));
        }
    }
}
impl Drop for Companion {
    fn drop(&mut self) {
        self.send.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
