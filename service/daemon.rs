use crate::{
    audio::{self, Capture},
    companion::Companion,
    config::{key_code, Config},
    engine::Engine,
    history::History,
    hotkey::{Hotkeys, KeyEvent},
    ipc::Server,
    output,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub enum Event {
    Connect(u64, mpsc::SyncSender<Value>),
    Disconnect(u64),
    Command(u64, Value),
    Key(KeyEvent),
    Level(u64, f32, f32),
    Model((String, String), Result<()>),
    Transcribed(u64, Result<String>, f64, Option<i64>),
    Delivered(u64, Result<()>),
    Test(u64, Value),
    Stop,
}
enum Work {
    Load(Config),
    Transcribe(std::path::PathBuf, Config, u64, Option<i64>),
}
#[derive(Default)]
struct Flow {
    generation: u64,
    pending: usize,
    recording: bool,
    outputting: bool,
    listening: bool,
}
impl Flow {
    fn busy(&self) -> bool {
        self.recording || self.pending > 0
    }
    fn can_record(&self) -> bool {
        self.listening && !self.recording && !self.outputting && self.pending < 4
    }
    fn cancel(&mut self) {
        self.generation += 1;
        self.recording = false;
    }
}
struct KeyCapture {
    client: u64,
    restore: bool,
    deadline: Instant,
}
struct MicTest {
    client: u64,
    token: u64,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
pub struct Daemon {
    server: Server,
    config: Config,
    history: History,
    clients: HashMap<u64, mpsc::SyncSender<Value>>,
    flow: Flow,
    capture: Option<Capture>,
    capture_id: u64,
    recording_client: Option<u64>,
    capture_ready: bool,
    recording_started: Instant,
    worker: mpsc::SyncSender<Work>,
    generation: Arc<AtomicU64>,
    hotkeys: Option<Hotkeys>,
    companion: Option<Companion>,
    ready: bool,
    last_error: String,
    resolved: String,
    completed: VecDeque<(u64, String)>,
    key_capture: Option<KeyCapture>,
    test: Option<MicTest>,
    test_serial: u64,
    quitting: bool,
}
impl Daemon {
    pub fn new(config: Config, probe: bool, probe_tray: bool) -> Result<Self> {
        if probe && std::env::var_os("STT_SOCKET_PATH").is_none() {
            bail!("Probe mode requires an isolated STT_SOCKET_PATH")
        }
        let server = Server::bind(crate::ipc::socket_path())?;
        Self::with_server(config, probe, probe_tray, server)
    }
    fn with_server(config: Config, probe: bool, probe_tray: bool, server: Server) -> Result<Self> {
        let history = History::open(&config.data_dir)?;
        let resolved = output::resolve(config.string("output", "method"));
        let (worker, receive) = mpsc::sync_channel::<Work>(5);
        let events = server.send.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        thread::spawn(move || {
            let mut engine: Option<Engine> = None;
            while let Ok(work) = receive.recv() {
                if let Work::Transcribe(_, _, generation, _) = &work {
                    if *generation != current.load(Ordering::Acquire) {
                        let _ = events.send(Event::Transcribed(
                            *generation,
                            Ok(String::new()),
                            0.,
                            None,
                        ));
                        continue;
                    }
                }
                let config = match &work {
                    Work::Load(c) | Work::Transcribe(_, c, _, _) => c,
                };
                let identity = (
                    config.string("transcription", "model").to_owned(),
                    config.string("transcription", "compute_type").to_owned(),
                );
                if !engine.as_ref().is_some_and(|e| e.matches(config)) {
                    let started = Instant::now();
                    log::info!("Loading speech model: {} / {}", identity.0, identity.1);
                    match Engine::load(config) {
                        Ok(loaded) => {
                            engine = Some(loaded);
                            log::info!(
                                "Speech model ready in {:.2}s",
                                started.elapsed().as_secs_f64()
                            );
                        }
                        Err(e) => {
                            match work {
                                Work::Load(_) => {
                                    let _ = events.send(Event::Model(identity, Err(e)));
                                }
                                Work::Transcribe(_, _, generation, history_id) => {
                                    let _ = events.send(Event::Transcribed(
                                        generation,
                                        Err(e),
                                        0.,
                                        history_id,
                                    ));
                                }
                            }
                            continue;
                        }
                    }
                }
                let _ = events.send(Event::Model(identity, Ok(())));
                if let Work::Transcribe(file, config, generation, history_id) = work {
                    let started = Instant::now();
                    let result =
                        crate::engine::read_audio(&file, config.flag("audio", "preprocess"))
                            .and_then(|samples| {
                                engine.as_ref().unwrap().transcribe(&samples, &config)
                            });
                    let _ = events.send(Event::Transcribed(
                        generation,
                        result,
                        started.elapsed().as_secs_f64(),
                        history_id,
                    ));
                }
            }
        });
        let hotkeys = if probe {
            None
        } else {
            let events = server.send.clone();
            Some(Hotkeys::start(
                key_code(config.string("input", "trigger_key"))
                    .unwrap()
                    .code(),
                move |key| {
                    let _ = events.send(Event::Key(key));
                },
            ))
        };
        let companion = if probe && !probe_tray {
            None
        } else {
            let (send, receive) = mpsc::sync_channel(16);
            let events = server.send.clone();
            thread::spawn(move || {
                while let Ok(command) = receive.recv() {
                    if events.send(Event::Command(0, command)).is_err() {
                        break;
                    }
                }
            });
            Some(Companion::start(send))
        };
        let daemon = Self {
            server,
            config,
            history,
            clients: HashMap::new(),
            flow: Flow {
                listening: true,
                ..Default::default()
            },
            capture: None,
            capture_id: 0,
            recording_client: None,
            capture_ready: false,
            recording_started: Instant::now(),
            worker,
            generation,
            hotkeys,
            companion,
            ready: false,
            last_error: String::new(),
            resolved,
            completed: VecDeque::new(),
            key_capture: None,
            test: None,
            test_serial: 0,
            quitting: false,
        };
        if !probe {
            daemon.worker.send(Work::Load(daemon.config.clone()))?;
        }
        Ok(daemon)
    }
    pub fn run(mut self) -> Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
        self.broadcast_state();
        while !stop.load(Ordering::Relaxed) && !self.quitting {
            match self.server.receive.recv_timeout(Duration::from_millis(200)) {
                Ok(Event::Stop) => break,
                Ok(event) => self.event(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            if self
                .key_capture
                .as_ref()
                .is_some_and(|k| Instant::now() > k.deadline)
            {
                if let Some(k) = &self.key_capture {
                    self.reply(k.client, json!({"type":"hotkey_timeout"}));
                }
                self.end_key_capture();
            }
            if self.flow.recording && self.recording_started.elapsed() > Duration::from_secs(600) {
                self.stop_recording();
                self.error("Recording stopped after ten minutes".into());
            }
            if self.capture.as_mut().is_some_and(Capture::exited) {
                self.stop_recording();
            }
        }
        self.flow.cancel();
        self.generation
            .store(self.flow.generation, Ordering::Release);
        self.capture.take();
        self.stop_test();
        self.end_key_capture();
        self.clients.clear();
        Ok(())
    }
    fn state(&self) -> Value {
        json!({"type":"state","protocol":2,"engine":"CTranslate2 / Rust","listening":self.flow.listening,"recording":self.flow.recording,"processing":self.flow.pending>0,"pending":self.flow.pending,"capture_ready":self.capture_ready,"model_ready":self.ready,"output_method":self.resolved,"last_error":self.last_error,"log_level":self.config.string("logging","level"),"hotkey":self.config.string("input","trigger_key"),"microphone":if self.config.string("audio","pipewire_node").is_empty(){self.config.string("audio","device")}else{self.config.string("audio","pipewire_node")}})
    }
    fn reply(&mut self, id: u64, value: Value) {
        if self
            .clients
            .get(&id)
            .is_some_and(|s| s.try_send(value).is_err())
        {
            self.clients.remove(&id);
        }
    }
    fn broadcast(&mut self, value: Value) {
        self.clients
            .retain(|_, client| client.try_send(value.clone()).is_ok());
    }
    fn broadcast_state(&mut self) {
        let state = self.state();
        self.broadcast(state.clone());
        if let Some(c) = &self.companion {
            c.update(state, &self.config);
        }
    }
    fn error(&mut self, message: String) {
        log::error!("{message}");
        self.last_error = message.clone();
        self.broadcast(json!({"type":"error","message":message}));
        if self.config.flag("notifications", "audio_feedback") {
            thread::spawn(|| {
                let _ = crate::process::run(
                    std::process::Command::new("paplay")
                        .arg("/usr/share/sounds/freedesktop/stereo/dialog-error.oga"),
                    None,
                    Duration::from_secs(5),
                );
            });
        }
        if self.config.flag("notifications", "enabled") {
            thread::spawn(move || {
                let _ = crate::process::run(
                    std::process::Command::new("notify-send").args([
                        "-u",
                        "critical",
                        "-t",
                        "5000",
                        "Voice Dictation",
                        &message,
                    ]),
                    None,
                    Duration::from_secs(5),
                );
            });
        }
    }
    fn event(&mut self, event: Event) {
        match event {
            Event::Connect(id, client) => {
                if self.clients.len() < 8 { self.clients.insert(id, client); self.reply(id, self.state()); }
            }
            Event::Disconnect(id) => {
                self.clients.remove(&id);
                if self.recording_client == Some(id) { self.discard_remote_recording(); }
                if self.key_capture.as_ref().is_some_and(|k| k.client == id) { self.end_key_capture(); }
                if self.test.as_ref().is_some_and(|t| t.client == id) { self.stop_test(); }
            }
            Event::Command(id, message) => {
                if let Err(error) = self.command(id, &message) {
                    self.reply(id, json!({"type":"error","message":error.to_string()}));
                    if id == 0 { self.error(error.to_string()); }
                }
            }
            Event::Key(KeyEvent::Down) => self.start_recording(),
            Event::Key(KeyEvent::Up) => {
                if self.recording_client.is_none() { self.stop_recording(); }
            },
            Event::Key(KeyEvent::Missing) => self.error("No readable keyboard supports the hotkey. Check the input group; waiting for a keyboard.".into()),
            Event::Key(KeyEvent::Captured(key)) => {
                if let Some(capture) = &self.key_capture {
                    self.reply(capture.client, json!({"type":"hotkey","key":key}));
                    self.end_key_capture();
                }
            }
            Event::Level(id, level, db) => {
                if self.flow.recording && id == self.capture_id {
                    self.capture_ready = true;
                    self.broadcast(json!({"type":"audio_level","level":level,"db":db,"capture_ready":true}));
                    let mut state = self.state(); state["level"] = json!(level);
                    if let Some(companion) = &self.companion { companion.update(state, &self.config); }
                }
            }
            Event::Model(identity, result) => {
                if identity != (self.config.string("transcription","model").to_owned(), self.config.string("transcription","compute_type").to_owned()) { return; }
                self.ready = result.is_ok();
                if let Err(error) = result { self.error(format!("Model load failed: {error}")); }
                self.broadcast_state();
            }
            Event::Transcribed(generation, result, duration, history_id) => self.transcribed_recording(generation, result, duration, history_id),
            Event::Delivered(_, result) => {
                self.flow.outputting = false;
                self.flow.pending = self.flow.pending.saturating_sub(1);
                if let Err(error) = result { self.error(error.to_string()); }
                self.flush_output(); self.broadcast_state();
            }
            Event::Test(token, message) => {
                if self.test.as_ref().is_some_and(|test| test.token == token) {
                    let id = self.test.as_ref().unwrap().client;
                    let done = message["done"] == true; self.reply(id, message);
                    if done { self.stop_test(); }
                }
            }
            Event::Stop => {}
        }
    }
    #[cfg(test)]
    fn transcribed(&mut self, generation: u64, result: Result<String>, duration: f64) {
        self.transcribed_recording(generation, result, duration, None);
    }
    fn transcribed_recording(
        &mut self,
        generation: u64,
        result: Result<String>,
        duration: f64,
        history_id: Option<i64>,
    ) {
        if generation != self.flow.generation {
            self.flow.pending = self.flow.pending.saturating_sub(1);
        } else {
            match result {
                Ok(text) if !text.is_empty() => {
                    log::info!(
                        "Transcription complete: characters={} elapsed={duration:.2}s",
                        text.chars().count()
                    );
                    let saved = if let Some(id) = history_id {
                        self.history.update_transcription(id, &text, false)
                    } else {
                        self.history.add(&text).map(|_| ())
                    };
                    if let Err(error) = saved {
                        self.error(format!("Could not save transcription history: {error}"));
                    }
                    self.broadcast(json!({"type":"transcription","text":text,"duration":duration}));
                    self.completed.push_back((generation, text));
                }
                result => {
                    self.flow.pending = self.flow.pending.saturating_sub(1);
                    let message = result
                        .err()
                        .map(|error| format!("Transcription failed: {error}"))
                        .unwrap_or_else(|| {
                            "No speech detected. Check the microphone in Settings.".into()
                        });
                    if let Some(id) = history_id {
                        let _ = self.history.update_transcription(id, "", true);
                    }
                    self.error(message);
                }
            }
        }
        self.flush_output();
        self.broadcast_state();
    }
    fn start_recording(&mut self) {
        self.start_recording_from(self.config.clone());
    }
    fn start_recording_from(&mut self, config: Config) {
        if !self.flow.can_record() {
            if self.flow.listening && !self.flow.recording {
                self.error("Wait for pending dictation or text output to finish.".into());
            }
            return;
        }
        if self.test.is_some() {
            self.error("Stop the microphone test before dictating.".into());
            return;
        }
        self.capture_id += 1;
        let id = self.capture_id;
        let send = self.server.send.clone();
        match Capture::start(&config, move |level, db| {
            let _ = send.try_send(Event::Level(id, level, db));
        }) {
            Ok(capture) => {
                self.capture = Some(capture);
                self.flow.recording = true;
                self.capture_ready = false;
                self.recording_started = Instant::now();
            }
            Err(e) => self.error(format!("Recording failed: {e}")),
        }
        self.broadcast_state();
    }
    fn stop_recording(&mut self) {
        if !self.flow.recording {
            return;
        }
        self.flow.recording = false;
        self.recording_client = None;
        self.capture_ready = false;
        if let Some(capture) = self.capture.take() {
            match capture.finish() {
                Ok(file) => {
                    let recordings = self.config.data_dir.join("recordings");
                    if let Err(error) = std::fs::create_dir_all(&recordings) {
                        self.error(format!("Could not save recording: {error}"));
                        return;
                    }
                    let destination = recordings.join(format!(
                        "dictation-{}-{}.wav",
                        self.capture_id,
                        std::process::id()
                    ));
                    let source = file.path().to_owned();
                    if let Err(error) = std::fs::copy(&source, &destination) {
                        self.error(format!("Could not save recording: {error}"));
                        return;
                    }
                    let duration = hound::WavReader::open(&destination)
                        .map(|reader| reader.duration() as f64 / reader.spec().sample_rate as f64)
                        .unwrap_or(0.);
                    let history_id = self
                        .history
                        .add_recording("", &destination, duration, true)
                        .ok();
                    self.flow.pending += 1;
                    if self
                        .worker
                        .try_send(Work::Transcribe(
                            destination,
                            self.config.clone(),
                            self.flow.generation,
                            history_id,
                        ))
                        .is_err()
                    {
                        self.flow.pending -= 1;
                        self.error("Transcription queue is full".into());
                    }
                }
                Err(e) => self.error(format!("Recording failed: {e}")),
            }
        }
        self.flush_output();
        self.broadcast_state();
    }
    fn discard_remote_recording(&mut self) {
        self.capture.take();
        self.flow.recording = false;
        self.recording_client = None;
        self.capture_ready = false;
        self.flush_output();
        self.broadcast_state();
    }
    fn cancel(&mut self) {
        self.recording_client = None;
        self.flow.cancel();
        self.generation
            .store(self.flow.generation, Ordering::Release);
        self.capture.take();
        self.capture_ready = false;
        self.flow.pending = self.flow.pending.saturating_sub(self.completed.len());
        self.completed.clear();
        self.broadcast(json!({"type":"cancelled"}));
        self.broadcast_state();
    }
    fn flush_output(&mut self) {
        if self.flow.recording || self.flow.outputting {
            return;
        }
        if let Some((generation, text)) = self.completed.pop_front() {
            if generation != self.flow.generation {
                self.flow.pending = self.flow.pending.saturating_sub(1);
                return;
            }
            self.flow.outputting = true;
            let config = self.config.clone();
            let resolved = self.resolved.clone();
            let events = self.server.send.clone();
            let current = self.generation.clone();
            thread::spawn(move || {
                let result = if generation == current.load(Ordering::Acquire) {
                    output::deliver(&text, &config, &resolved)
                } else {
                    Ok(())
                };
                let _ = events.send(Event::Delivered(generation, result));
            });
        }
    }
    fn config_reply(&mut self, id: u64) {
        self.reply(
            id,
            json!({"type":"config","config":self.config.data,"microphones":audio::microphones()}),
        );
    }
    fn command(&mut self, id: u64, message: &Value) -> Result<()> {
        match message["cmd"].as_str().unwrap_or("") {
            "start_recording" => {
                if id == 0 || !self.clients.contains_key(&id) {
                    bail!("Recording requires a connected client")
                }
                if !self.flow.can_record() || self.key_capture.is_some() || self.test.is_some() {
                    bail!("Dictation is paused or busy")
                }
                let node = message["pipewire_node"].as_str().unwrap_or("");
                if node.is_empty() || node.len() > 256 || node.chars().any(char::is_control) {
                    bail!("A microphone node is required")
                }
                let config = self.config.changed(&json!({"audio":{
                    "device":"pipewire", "pipewire_node":node
                }}))?;
                self.start_recording_from(config);
                if !self.flow.recording {
                    bail!("Could not start recording: {}", self.last_error)
                }
                self.recording_client = Some(id);
                self.reply(id, json!({"type":"recording_started"}));
            }
            "stop_recording" | "abort_recording" => {
                if self.recording_client != Some(id) {
                    bail!("This client does not own the recording")
                }
                if message["cmd"] == "abort_recording" {
                    self.discard_remote_recording();
                } else {
                    self.stop_recording();
                }
                self.reply(id, json!({"type":"recording_stopped"}));
            }
            "get_state" => self.reply(id, self.state()),
            "get_config" => {
                self.config_reply(id);
                if let Some(item) = self.history.recent(1, "")?.first() {
                    self.reply(id, json!({"type":"transcription","text":item["text"]}));
                }
            }
            "history" => {
                let query = message["search"].as_str().unwrap_or("");
                if query.len() > 4096 {
                    bail!("Search is too long")
                }
                self.reply(id,json!({"type":"history","search":query,"items":self.history.recent(100,query)?}));
            }
            "favorite_history" => {
                self.history.set_favorite(
                    message["id"].as_i64().unwrap_or_default(),
                    message["favorite"].as_bool().unwrap_or(false),
                )?;
                self.reply(id, json!({"type":"history_changed"}));
            }
            "delete_history" => {
                if let Some(path) = self
                    .history
                    .remove(message["id"].as_i64().unwrap_or_default())?
                {
                    match std::fs::remove_file(path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                self.reply(id, json!({"type":"history_changed"}));
            }
            "retry_history" => {
                let history_id = message["id"].as_i64().unwrap_or_default();
                let item = self
                    .history
                    .get(history_id)?
                    .ok_or_else(|| anyhow::anyhow!("Recording no longer exists"))?;
                let path = item["audio_path"]
                    .as_str()
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("This older history item has no saved recording")
                    })?;
                if !std::path::Path::new(path).is_file() {
                    bail!("The saved recording is missing")
                }
                self.flow.pending += 1;
                if self
                    .worker
                    .try_send(Work::Transcribe(
                        path.into(),
                        self.config.clone(),
                        self.flow.generation,
                        Some(history_id),
                    ))
                    .is_err()
                {
                    self.flow.pending -= 1;
                    bail!("Transcription queue is full")
                }
                self.broadcast_state();
            }
            "play_history" => {
                if let Some(item) = self
                    .history
                    .get(message["id"].as_i64().unwrap_or_default())?
                {
                    if let Some(path) = item["audio_path"].as_str() {
                        std::process::Command::new("xdg-open")
                            .arg(path)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn()?;
                    }
                }
            }
            "open_recordings" => {
                let directory = self.config.data_dir.join("recordings");
                std::fs::create_dir_all(&directory)?;
                std::process::Command::new("xdg-open")
                    .arg(directory)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()?;
            }
            "toggle_listening" | "set_listening" => {
                if self.key_capture.is_some() {
                    bail!("Finish hotkey capture first")
                }
                self.flow.listening = if message["cmd"] == "toggle_listening" {
                    !self.flow.listening
                } else {
                    message["value"]
                        .as_bool()
                        .ok_or_else(|| anyhow::anyhow!("Expected a boolean"))?
                };
                if !self.flow.listening {
                    self.stop_recording();
                }
                self.broadcast_state();
            }
            "cancel" => self.cancel(),
            "clear_error" => {
                self.last_error.clear();
                self.broadcast_state();
            }
            "save_config" | "reload_config" | "set_log_level" => {
                if self.flow.busy() || self.key_capture.is_some() || self.test.is_some() {
                    bail!("Wait for dictation or microphone/hotkey testing to finish, then save settings again.")
                }
                let candidate = if message["cmd"] == "reload_config" {
                    Config::load()?
                } else if message["cmd"] == "set_log_level" {
                    self.config
                        .changed(&json!({"logging":{"level":message["value"]}}))?
                } else {
                    self.config.changed(&message["config"])?
                };
                let model_changed = candidate.string("transcription", "model")
                    != self.config.string("transcription", "model")
                    || candidate.string("transcription", "compute_type")
                        != self.config.string("transcription", "compute_type");
                let resolved = output::resolve(candidate.string("output", "method"));
                if model_changed {
                    self.worker
                        .try_send(Work::Load(candidate.clone()))
                        .map_err(|_| {
                            anyhow::anyhow!("Model loading is busy. Try saving again shortly.")
                        })?;
                }
                candidate.save()?;
                self.config = candidate;
                self.resolved = resolved;
                crate::logging::level(&self.config);
                if let Some(keys) = &self.hotkeys {
                    keys.key.store(
                        key_code(self.config.string("input", "trigger_key"))
                            .unwrap()
                            .code(),
                        Ordering::Relaxed,
                    );
                }
                if model_changed {
                    self.ready = false;
                }
                self.broadcast(json!({"type":"config_reloaded"}));
                self.config_reply(id);
                self.broadcast_state();
            }
            "capture_hotkey" => {
                if self.flow.busy() || self.key_capture.is_some() || self.test.is_some() {
                    bail!("Finish dictation or testing before changing the hotkey")
                }
                let keys = self
                    .hotkeys
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hotkeys are disabled in probe mode"))?;
                self.key_capture = Some(KeyCapture {
                    client: id,
                    restore: self.flow.listening,
                    deadline: Instant::now() + Duration::from_secs(10),
                });
                self.flow.listening = false;
                keys.capture.store(true, Ordering::Relaxed);
                self.reply(id, json!({"type":"hotkey_waiting"}));
                self.broadcast_state();
            }
            "test_microphone" => {
                if self.flow.busy() || self.key_capture.is_some() {
                    bail!("Finish dictation before testing the microphone")
                }
                if self.test.is_some() {
                    self.stop_test();
                    self.reply(id,json!({"type":"microphone_test","level":0,"message":"Microphone test stopped.","done":true}));
                } else {
                    self.start_test(
                        id,
                        message["node"]
                            .as_str()
                            .unwrap_or(self.config.string("audio", "pipewire_node"))
                            .to_owned(),
                    );
                }
            }
            "diagnostics" => {
                let tools = ["arecord", "ffmpeg", "xdotool", "ydotool", "dotool", "wtype"]
                    .map(|tool| json!({"name":tool,"installed":crate::process::exists(tool)}));
                let text = json!({"session":std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),"socket":crate::ipc::socket_path(),"daemon":self.state(),"audio":self.config.data["audio"],"transcription":self.config.data["transcription"],"microphones":audio::microphones(),"engine":"CTranslate2 via ct2rs 0.10.1 (Rust)","tools":tools});
                self.reply(id,json!({"type":"diagnostics","text":serde_json::to_string_pretty(&text)?,"logs":crate::logging::recent(&self.config)}));
            }
            "quit" => {
                // Existing adapters must not automatically restart an intentional quit.
                std::fs::write(crate::ipc::stopped_path(), b"")?;
                self.quitting = true;
            }
            _ => bail!("Unknown speech service command"),
        }
        Ok(())
    }
    fn end_key_capture(&mut self) {
        if let Some(capture) = self.key_capture.take() {
            self.flow.listening = capture.restore;
            if let Some(keys) = &self.hotkeys {
                keys.capture.store(false, Ordering::Relaxed);
            }
            self.broadcast_state();
        }
    }
    fn stop_test(&mut self) {
        if let Some(test) = self.test.take() {
            test.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = test.thread {
                let _ = thread.join();
            }
        }
    }
    fn start_test(&mut self, id: u64, node: String) {
        let stop = Arc::new(AtomicBool::new(false));
        self.test_serial += 1;
        let token = self.test_serial;
        self.test = Some(MicTest {
            client: id,
            token,
            stop: stop.clone(),
            thread: None,
        });
        let events = self.server.send.clone();
        let device = self.config.string("audio", "device").to_owned();
        let thread = thread::spawn(move || {
            let send = |level: f32, message: String, done: bool| {
                let _ = events.try_send(Event::Test(
                    token,
                    json!({"type":"microphone_test","level":level,"message":message,"done":done}),
                ));
            };
            let result = (|| -> Result<()> {
                use std::{
                    io::Read,
                    process::{Command, Stdio},
                };
                let mut command = Command::new("arecord");
                command
                    .args([
                        "-q",
                        "-D",
                        if node.is_empty() { &device } else { "pipewire" },
                        "-t",
                        "raw",
                        "-f",
                        "S16_LE",
                        "-r",
                        "16000",
                        "-c",
                        "1",
                        "-d",
                        "8",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                if !node.is_empty() {
                    command.env("PIPEWIRE_NODE", node);
                }
                crate::process::kill_with_parent(&mut command);
                let mut child = command.spawn()?;
                let mut stream = child.stdout.take().unwrap();
                if let Err(error) = crate::process::nonblocking(&stream) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
                let started = Instant::now();
                let mut peak = 0.;
                let mut block = [0; 3200];
                while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(10) {
                    match stream.read(&mut block) {
                        Ok(0) => break,
                        Ok(n) => {
                            let (level, db) = audio::level(&block[..n]);
                            if level > peak {
                                peak = level;
                            }
                            send(
                                level,
                                format!("{db:.0} dB · Speak normally to check your microphone"),
                                false,
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(50))
                        }
                        Err(_) => break,
                    }
                }
                let mut status = child.try_wait()?;
                let deadline = Instant::now() + Duration::from_millis(200);
                while status.is_none() && !stop.load(Ordering::Relaxed) && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(10));
                    status = child.try_wait()?;
                }
                if status.is_none() {
                    let _ = child.kill();
                }
                let _ = child.wait();
                if !stop.load(Ordering::Relaxed) {
                    if !status.is_some_and(|s| s.success()) {
                        bail!("Microphone test failed or timed out")
                    }
                    send(
                        0.,
                        if peak > 0. {
                            "Microphone signal detected. You are ready to dictate."
                        } else {
                            "No signal. Check microphone selection and mute."
                        }
                        .into(),
                        true,
                    );
                }
                Ok(())
            })();
            if let Err(e) = result {
                send(0., format!("Microphone test failed: {e}"), true);
            }
        });
        self.test.as_mut().unwrap().thread = Some(thread);
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        self.capture.take();
        self.stop_test();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn isolated() -> (tempfile::TempDir, Daemon) {
        let root = tempfile::tempdir().unwrap();
        let config = Config::at(root.path())
            .unwrap()
            .changed(&json!({
                "output":{"method":"none"},
                "notifications":{"enabled":false,"audio_feedback":false}
            }))
            .unwrap();
        let server = Server::bind(root.path().join("run/daemon.sock")).unwrap();
        let daemon = Daemon::with_server(config, true, false, server).unwrap();
        (root, daemon)
    }
    #[test]
    fn canceled_inference_never_reaches_history_or_output() {
        let (_root, mut daemon) = isolated();
        daemon.flow.pending = 1;
        daemon.cancel();
        daemon.transcribed(0, Ok("Synthetic canceled words".into()), 0.1);
        assert!(daemon.history.recent(100, "").unwrap().is_empty());
        assert_eq!(daemon.flow.pending, 0);
        assert!(!daemon.flow.outputting);
    }
    #[test]
    fn completed_dictations_wait_for_release_and_keep_order() {
        let (_root, mut daemon) = isolated();
        daemon.flow.recording = true;
        daemon.flow.pending = 2;
        daemon.transcribed(0, Ok("Synthetic first".into()), 0.1);
        daemon.transcribed(0, Ok("Synthetic second".into()), 0.1);
        assert!(!daemon.flow.outputting);
        assert_eq!(
            daemon
                .completed
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>(),
            ["Synthetic first", "Synthetic second"]
        );
        daemon.flow.recording = false;
        daemon.flush_output();
        assert!(daemon.flow.outputting);
        assert_eq!(daemon.completed.front().unwrap().1, "Synthetic second");
        for _ in 0..2 {
            let event = daemon
                .server
                .receive
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            assert!(matches!(event, Event::Delivered(_, _)));
            daemon.event(event);
        }
        assert_eq!(daemon.flow.pending, 0);
        assert!(!daemon.flow.outputting);
    }
    #[test]
    fn output_failure_keeps_history_and_allows_next_dictation() {
        let (_root, mut daemon) = isolated();
        daemon.config.data["output"]["method"] = json!("auto");
        daemon.flow.pending = 1;
        daemon.transcribed(0, Ok("Synthetic retained text".into()), 0.1);
        assert_eq!(
            daemon.history.recent(1, "").unwrap()[0]["text"],
            "Synthetic retained text"
        );
        let event = daemon
            .server
            .receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        daemon.event(event);
        assert!(daemon.last_error.contains("History"));
        assert_eq!(daemon.flow.pending, 0);
        assert!(daemon.flow.can_record());
    }
    #[test]
    fn busy_settings_never_write_and_reload_keeps_pause() {
        let (_root, mut daemon) = isolated();
        daemon.flow.pending = 1;
        assert!(daemon
            .command(
                0,
                &json!({"cmd":"save_config","config":{"ui":{"cursor_indicator":true}}})
            )
            .is_err());
        assert!(!daemon.config.path.exists());
        daemon.flow.pending = 0;
        daemon.flow.listening = false;
        daemon
            .command(
                0,
                &json!({"cmd":"save_config","config":{"ui":{"cursor_indicator":true}}}),
            )
            .unwrap();
        assert!(!daemon.flow.listening);
        assert!(daemon.config.flag("ui", "cursor_indicator"));
    }
    #[test]
    fn remote_recording_requires_owner_and_ignores_hotkey_release() {
        let (_root, mut daemon) = isolated();
        daemon.flow.recording = true;
        daemon.recording_client = Some(7);
        assert!(daemon.command(8, &json!({"cmd":"stop_recording"})).is_err());
        daemon.event(Event::Key(KeyEvent::Up));
        assert!(daemon.flow.recording);
        daemon.command(7, &json!({"cmd":"stop_recording"})).unwrap();
        assert!(!daemon.flow.recording);
        assert_eq!(daemon.recording_client, None);
    }
    #[test]
    fn remote_disconnect_discards_only_its_recording() {
        let (_root, mut daemon) = isolated();
        daemon.flow.recording = true;
        daemon.flow.pending = 1;
        daemon.recording_client = Some(7);
        daemon.event(Event::Disconnect(8));
        assert!(daemon.flow.recording);
        daemon.event(Event::Disconnect(7));
        assert!(!daemon.flow.recording);
        assert_eq!(daemon.flow.pending, 1);
        assert_eq!(daemon.flow.generation, 0);
    }
    #[test]
    fn remote_start_respects_pause_and_existing_recording() {
        let (_root, mut daemon) = isolated();
        let (send, _receive) = mpsc::sync_channel(64);
        daemon.clients.insert(7, send);
        let request = json!({"cmd":"start_recording", "pipewire_node":"phonemic2_src"});
        daemon.flow.listening = false;
        assert!(daemon.command(7, &request).is_err());
        daemon.flow.listening = true;
        daemon.flow.recording = true;
        assert!(daemon.command(7, &request).is_err());
        assert_eq!(daemon.recording_client, None);
        assert!(!daemon.config.path.exists());
    }
    #[test]
    fn queue_is_bounded_and_typing_blocks_new_capture() {
        let mut f = Flow {
            listening: true,
            ..Default::default()
        };
        assert!(f.can_record());
        f.pending = 4;
        assert!(!f.can_record());
        f.pending = 1;
        f.outputting = true;
        assert!(!f.can_record());
        f.outputting = false;
        assert!(f.can_record());
    }
    #[test]
    fn cancel_invalidates_inflight_generation() {
        let mut f = Flow {
            recording: true,
            generation: 9,
            pending: 2,
            ..Default::default()
        };
        f.cancel();
        assert_eq!(f.generation, 10);
        assert!(!f.recording);
        assert_eq!(f.pending, 2);
    }
}
