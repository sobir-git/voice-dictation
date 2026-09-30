#[path = "commands.rs"]
mod commands;
#[path = "events.rs"]
mod events;
#[path = "worker.rs"]
mod worker;
use crate::{
    audio::{self, Capture},
    companion::Companion,
    config::{key_code, Config},
    engine::{StreamInput, PARAKEET_MODEL},
    history::History,
    hotkey::{Hotkeys, KeyEvent},
    ipc::Server,
    jobs::{JobId, Jobs, Stage, Transcript},
    optimization, output,
    storage::{SavedRecording, Storage},
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
pub enum Event {
    Connect(u64, mpsc::SyncSender<Value>),
    Disconnect(u64),
    Command(u64, Value),
    Key(KeyEvent),
    Level(JobId, f32, f32),
    Preview(JobId, String, String),
    Model((String, String), Result<String>),
    WorkerUnavailable((String, String)),
    ModelLoading((String, String)),
    CaptureStarted(JobId, Result<Capture>),
    CaptureFault(JobId, String),
    Finalized(JobId, Result<(std::path::PathBuf, Option<String>)>),
    Saved(JobId, Result<SavedRecording>),
    #[cfg(test)]
    Transcribed(JobId, Transcript),
    WorkerTranscribed(JobId, Arc<AtomicBool>, Transcript),
    Persisted(JobId, Result<()>),
    Delivered(JobId, Result<()>),
    Storage(Option<u64>, Box<Event>),
    Reply(u64, Result<Value>),
    Exported(u64, Option<u64>, Result<Value>),
    RetryReady(JobId, Result<std::path::PathBuf>),
    ConfigSaved(u64, Result<(Config, String)>),
    Test(u64, Value),
    Benchmark(Value),
    BenchmarkDone(Result<Value>),
    Stop,
}
enum Work {
    Benchmark(Config, Vec<u64>, Arc<AtomicBool>),
    Load(Config),
    Transcribe {
        id: JobId,
        generation: u64,
        cancelled: Arc<AtomicBool>,
        path: std::path::PathBuf,
        config: Config,
        requested: Instant,
        queued: Instant,
    },
    Stream {
        id: JobId,
        generation: u64,
        cancelled: Arc<AtomicBool>,
        config: Config,
        receive: mpsc::Receiver<StreamInput>,
        queued: Instant,
        finished: Arc<OnceLock<Instant>>,
    },
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelState {
    Unavailable,
    Loading,
    Ready,
}
pub struct Daemon {
    server: Server,
    config: Config,
    storage: Storage,
    io_pending: usize,
    configuring: bool,
    exporting: bool,
    request_id: Option<u64>,
    #[cfg(test)]
    history: History,
    clients: HashMap<u64, mpsc::SyncSender<Value>>,
    jobs: Jobs,
    worker: mpsc::SyncSender<Work>,
    generation: Arc<AtomicU64>,
    worker_shutdown: Arc<AtomicBool>,
    hotkeys: Option<Hotkeys>,
    companion: Option<Companion>,
    model_state: ModelState,
    last_error: String,
    resolved: String,
    key_capture: Option<KeyCapture>,
    test: Option<MicTest>,
    test_serial: u64,
    quitting: bool,
    benchmarking: bool,
    benchmark_cancel: Arc<AtomicBool>,
    benchmark_status: Value,
    benchmark_client: Option<u64>,
    runtime_profile: String,
}
impl Daemon {
    pub fn new(config: Config, probe: bool, probe_tray: bool) -> Result<Self> {
        if probe && std::env::var_os("STT_SOCKET_PATH").is_none() {
            bail!("Probe mode requires an isolated STT_SOCKET_PATH");
        }
        Self::with_server(
            config,
            probe,
            probe_tray,
            Server::bind(crate::ipc::socket_path())?,
        )
    }
    fn with_server(config: Config, probe: bool, probe_tray: bool, server: Server) -> Result<Self> {
        let history_owner = History::open(&config.data_dir)?;
        // Startup only, before capture admission. All later storage runs on its worker.
        if let Err(error) =
            crate::storage::recover_recordings(&history_owner, &config.data_dir.join("recordings"))
        {
            log::error!("Recording recovery scan failed; retained files remain on disk: {error:#}");
        }
        let storage = Storage::start(history_owner, server.send.clone());
        #[cfg(test)]
        let history = History::open(&config.data_dir)?;
        let resolved = output::resolve(config.string("output", "method"));
        let (worker, receive) = mpsc::sync_channel(5);
        let generation = Arc::new(AtomicU64::new(0));
        let worker_shutdown = Arc::new(AtomicBool::new(false));
        worker::start_worker(
            receive,
            server.send.clone(),
            generation.clone(),
            worker_shutdown.clone(),
        );
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
        let mut daemon = Self {
            server,
            config,
            storage,
            io_pending: 0,
            configuring: false,
            exporting: false,
            request_id: None,
            #[cfg(test)]
            history,
            clients: HashMap::new(),
            jobs: Jobs::listening(),
            worker,
            generation,
            worker_shutdown,
            hotkeys,
            companion,
            model_state: if probe {
                ModelState::Unavailable
            } else {
                ModelState::Loading
            },
            last_error: String::new(),
            resolved,
            key_capture: None,
            test: None,
            test_serial: 0,
            quitting: false,
            benchmarking: false,
            benchmark_cancel: Arc::new(AtomicBool::new(false)),
            benchmark_client: None,
            runtime_profile: String::new(),
            benchmark_status: json!({
            "type":"benchmark","running":false,"message":"Choose a configuration and run a local benchmark."}
            ),
        };
        if !probe {
            let config = daemon.config.clone();
            daemon.effect(move |history| {
                if let Err(error) = crate::storage::prune_recordings(history, &config, None) {
                    log::warn!("Recording retention failed: {error:#}");
                }
                Event::Reply(0, Ok(json!({"type":"maintenance_finished"})))
            })?;
            daemon
                .worker
                .try_send(Work::Load(daemon.config.clone()))
                .map_err(|_| anyhow::anyhow!("Speech worker unavailable"))?;
        }
        Ok(daemon)
    }
    fn effect(&mut self, task: impl FnOnce(&History) -> Event + Send + 'static) -> Result<()> {
        self.storage.submit(self.request_id, task)?;
        self.io_pending += 1;
        Ok(())
    }
    pub fn run(mut self) -> Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
        self.broadcast_state();
        while !stop.load(Ordering::Relaxed) && !self.quitting {
            match self.server.receive.recv_timeout(Duration::from_millis(100)) {
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
                    self.reply(
                        k.client,
                        json!({
                        "type":"hotkey_timeout"}
                        ),
                    );
                }
                self.end_key_capture();
            }
            if let Some(id) = self.jobs.recording() {
                let job = self.jobs.active.get_mut(&id).unwrap();
                if job.started.elapsed() > Duration::from_secs(600) {
                    self.stop_recording();
                    self.error("Recording stopped after ten minutes".into());
                } else if job.capture.as_mut().is_some_and(Capture::exited) {
                    self.stop_recording();
                }
            }
        }
        self.finish_shutdown();
        self.cancel();
        self.stop_test();
        self.end_key_capture();
        self.clients.clear();
        Ok(())
    }
    fn finish_shutdown(&mut self) {
        self.stop_recording();
        let deadline = Instant::now() + Duration::from_secs(8);
        while (self.jobs.busy() || self.io_pending > 0) && Instant::now() < deadline {
            match self.server.receive.recv_timeout(Duration::from_millis(50)) {
                // Shutdown drains admitted effects, never admits more user work.
                Ok(Event::Command(..) | Event::Key(..) | Event::Connect(..)) => {}
                Ok(event) => self.event(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        if self.jobs.busy() {
            log::warn!(
                "Shutdown deadline: unfinished_jobs={}",
                self.jobs.active.len()
            );
        }
    }
    fn state(&self) -> Value {
        let recording = self
            .jobs
            .recording()
            .and_then(|id| self.jobs.active.get(&id));
        let jobs: Vec<_> = self
            .jobs
            .active
            .values()
            .map(|j| {
                json!({
                "id":j.id,"stage":j.stage,"history_id":j.history_id}
                )
            })
            .collect();
        json!({
        "type":"state","protocol":2,"daemon_session":crate::logging::session(),"daemon_pid":std::process::id(),"engine":"Rust speech engines","listening":self.jobs.listening,"recording":recording.is_some(),"processing":self.jobs.pending()>0,"pending":self.jobs.pending(),"capture_ready":recording.is_some_and(|j|j.capture_ready),"model_ready":self.model_state==ModelState::Ready,"model_loading":self.model_state==ModelState::Loading,"benchmarking":self.benchmarking,"configuring":self.configuring,"runtime_profile":self.runtime_profile,"performance":self.config.data["performance"],"model":self.config.string("transcription","model"),"output_method":self.resolved,"last_error":self.last_error,"log_level":self.config.string("logging","level"),"hotkey":self.config.string("input","trigger_key"),"jobs":jobs,"microphone":if self.config.string("audio","pipewire_node").is_empty(){
        self.config.string("audio","device")}
        else{
        self.config.string("audio","pipewire_node")}
        }
        )
    }
    fn reply(&mut self, id: u64, mut value: Value) {
        if let Some(request_id) = self.request_id {
            value["request_id"] = json!(request_id);
        }
        let _context =
            crate::logging::context(json!({"client_id":id,"request_id":value["request_id"]}));
        log::info!(
            "Reply: client={id} request_id={} type={}",
            value["request_id"],
            value["type"]
        );
        if self
            .clients
            .get(&id)
            .is_some_and(|s| s.try_send(value).is_err())
        {
            log::warn!("Client disconnected: client={id} reason=reply_queue_full");
            self.disconnect(id);
        }
    }
    fn broadcast(&mut self, value: Value) {
        let gone: Vec<_> = self
            .clients
            .iter()
            .filter_map(|(id, c)| c.try_send(value.clone()).is_err().then_some(*id))
            .collect();
        for id in gone {
            log::warn!("Client disconnected: client={id} reason=broadcast_queue_full");
            self.disconnect(id);
        }
    }
    fn disconnect(&mut self, id: u64) {
        log::info!("Client disconnected: client={id}");
        self.clients.remove(&id);
        if self.benchmark_client == Some(id) {
            self.benchmark_cancel.store(true, Ordering::Relaxed);
        }
        if let Some(job) = self
            .jobs
            .recording()
            .filter(|job| self.jobs.active[job].owner == Some(id))
        {
            log::warn!(
                "Recording owner disconnected: job={job}; saving and transcribing captured audio"
            );
            self.stop_recording();
        }
        if self.key_capture.as_ref().is_some_and(|k| k.client == id) {
            self.end_key_capture();
        }
        if self.test.as_ref().is_some_and(|t| t.client == id) {
            self.stop_test();
        }
    }
    fn broadcast_state(&mut self) {
        let state = self.state();
        log::info!(
            "State: generation={} listening={} recording={} pending={} outputting={}",
            self.jobs.generation,
            self.jobs.listening,
            self.jobs.recording().is_some(),
            self.jobs.pending(),
            self.jobs.outputting()
        );
        self.broadcast(state.clone());
        if let Some(c) = &self.companion {
            c.update(state, &self.config);
        }
    }
    fn error(&mut self, message: String) {
        log::error!("{message}");
        self.last_error = message.clone();
        self.broadcast(json!({
        "type":"error","message":message}
        ));
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
    fn fail(&mut self, id: JobId, message: String) {
        if !self.jobs.active.contains_key(&id) {
            return;
        }
        let job = &self.jobs.active[&id];
        let _context = crate::logging::context(json!({
            "event":"dictation_failed", "job_id":id, "history_id":job.history_id,
            "stage":job.stage, "model":job.config.string("transcription","model"),
            "requested_profile":optimization::profile(&job.config), "runtime_profile":self.runtime_profile,
            "elapsed_ms":job.started.elapsed().as_millis(), "error":message
        }));
        log::error!("Dictation failed: job={id} history={:?} stage={:?} runtime={} elapsed_ms={} error={message}",job.history_id,job.stage,self.runtime_profile,job.started.elapsed().as_millis());
        self.terminal(id, "failed");
        self.error(message);
        self.flush_output();
        self.broadcast_state();
    }
    fn terminal(&mut self, id: JobId, outcome: &str) {
        if let Some(mut job) = self.jobs.complete(id, outcome) {
            job.cancelled.store(true, Ordering::Release);
            if let Some(stream) = job.stream.take() {
                let _ = stream.try_send(StreamInput::Cancel);
            }
            if let Some(capture) = job.capture.take() {
                self.finish_capture(id, capture);
            }
        }
    }
    fn persist(&mut self, id: JobId) {
        let Some(job) = self.jobs.active.get_mut(&id) else {
            return;
        };
        let Some(result) = &job.result else {
            return;
        };
        let history_id = job.history_id;
        let text = result.text.as_ref().cloned().unwrap_or_default();
        let failed = result.text.is_err() || text.is_empty();
        let model = result.model.clone();
        let seconds = result.seconds;
        let cancelled = job.cancelled.clone();
        job.transition(Stage::Persisting);
        if let Err(e) = self.effect(move |history| {
            if cancelled.load(Ordering::Acquire) {
                return Event::Persisted(id, Ok(()));
            }
            let result = if let Some(id) = history_id {
                history.finish_transcription(id, &text, failed, &model, seconds)
            } else if !failed {
                history
                    .add_transcription(&text, &model, seconds)
                    .map(|_| ())
            } else {
                Ok(())
            };
            Event::Persisted(id, result)
        }) {
            self.event(Event::Persisted(id, Err(e)));
        }
    }
    fn start_recording(&mut self) {
        self.start_recording_from(self.config.clone(), None);
    }
    fn start_recording_from(&mut self, config: Config, owner: Option<u64>) {
        if self.benchmarking || self.configuring || !self.jobs.can_record() {
            return;
        }
        if self.test.is_some() {
            self.error("Stop the microphone test before dictating.".into());
            return;
        }
        let id = self
            .jobs
            .insert(config.clone(), Stage::Starting, owner, None);
        let model = config.string("transcription", "model");
        let streaming = (model == PARAKEET_MODEL || model.ends_with(".gguf"))
            && optimization::profile(&config) != "vulkan-full"
            && config.number("audio", "sample_rate") == 16000
            && config.number("audio", "channels") == 1
            && config.string("audio", "format") == "S16_LE";
        let (audio, receive) = mpsc::sync_channel(256);
        if streaming {
            let job = self.jobs.active.get_mut(&id).unwrap();
            job.stream = Some(audio.clone());
            if self
                .worker
                .try_send(Work::Stream {
                    id,
                    generation: job.generation,
                    cancelled: job.cancelled.clone(),
                    config: config.clone(),
                    receive,
                    queued: Instant::now(),
                    finished: job.finished.clone(),
                })
                .is_err()
            {
                self.fail(id, "Speech worker is busy".into());
                return;
            }
        }
        let send = self.server.send.clone();
        thread::spawn(move || {
            let events = send.clone();
            let overrun = AtomicBool::new(false);
            let accepted_samples = AtomicU64::new(0);
            let accepted_chunks = AtomicU64::new(0);
            let capture_started = Instant::now();
            let (owner, owner_lifetime) = mpsc::channel();
            let result = Capture::start(&config, move |samples, level, db| {
                if streaming && !overrun.load(Ordering::Relaxed) {
                    for chunk in samples.chunks(1600) {
                        match audio.try_send(StreamInput::audio(chunk.to_vec())) {
                            Ok(()) => {
                                accepted_samples.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                                accepted_chunks.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => {
                                let reason = match error {
                                    mpsc::TrySendError::Full(_) => "queue_full",
                                    mpsc::TrySendError::Disconnected(_) => "worker_disconnected",
                                };
                                let _context = crate::logging::context(json!({
                                    "event":"stream_delivery_failed", "job_id":id, "reason":reason,
                                    "queue_capacity_chunks":256, "accepted_chunks":accepted_chunks.load(Ordering::Relaxed),
                                    "accepted_samples":accepted_samples.load(Ordering::Relaxed),
                                    "callback_samples":samples.len(), "rejected_chunk_samples":chunk.len(),
                                    "capture_elapsed_ms":capture_started.elapsed().as_millis()
                                }));
                                log::error!("Stream delivery failed: job={id} reason={reason} capacity_chunks=256 accepted_chunks={} accepted_audio_seconds={:.3} capture_elapsed_ms={} recovery=transcribe_saved_audio",accepted_chunks.load(Ordering::Relaxed),accepted_samples.load(Ordering::Relaxed) as f64 / 16000.,capture_started.elapsed().as_millis());
                                overrun.store(true, Ordering::Relaxed);
                                let _=events.send(Event::CaptureFault(id,format!("Streaming interrupted ({reason}); recovering the saved recording.")));
                                break;
                            }
                        }
                    }
                }
                let _ = events.try_send(Event::Level(id, level, db));
            });
            let result = result.map(|mut capture| {
                capture.retain_spawn_owner(owner);
                capture
            });
            if let Err(error) = send.send(Event::CaptureStarted(id, result)) {
                if let Event::CaptureStarted(_, Ok(capture)) = error.0 {
                    drop(capture);
                }
            }
            let _ = owner_lifetime.recv();
        });
        self.last_error.clear();
        self.broadcast_state();
    }
    fn stop_recording(&mut self) {
        let Some(id) = self.jobs.recording() else {
            return;
        };
        let job = self.jobs.active.get_mut(&id).unwrap();
        let _ = job.finished.set(Instant::now());
        job.transition(Stage::Finalizing);
        self.finalize_capture(id);
        self.broadcast_state();
    }
    fn finalize_capture(&mut self, id: JobId) {
        let Some(capture) = self.jobs.active.get_mut(&id).and_then(|j| j.capture.take()) else {
            return;
        };
        self.finish_capture(id, capture);
    }
    fn finish_capture(&self, id: JobId, capture: Capture) {
        let send = self.server.send.clone();
        thread::spawn(move || {
            let (file, warning) = capture.finish();
            // Cleanup is disabled by Capture; dropping this handle never unlinks audio.
            let path = file.path().to_owned();
            drop(file);
            let result = Ok((path, warning));
            let _ = send.send(Event::Finalized(id, result));
        });
    }
    fn cancel_remote_recording(&mut self) {
        if let Some(id) = self.jobs.recording() {
            self.terminal(id, "aborted");
        }
        self.flush_output();
        self.broadcast_state();
    }
    fn cancel(&mut self) {
        self.jobs.generation += 1;
        self.generation
            .store(self.jobs.generation, Ordering::Release);
        for job in self.jobs.active.values() {
            job.cancelled.store(true, Ordering::Release);
        }
        let ids: Vec<_> = self
            .jobs
            .active
            .values()
            .filter(|j| j.stage != Stage::Delivering)
            .map(|j| j.id)
            .collect();
        for id in ids {
            self.terminal(id, "cancelled");
        }
        self.broadcast(json!({
        "type":"cancelled"}
        ));
        self.broadcast_state();
    }
    fn flush_output(&mut self) {
        if self.jobs.recording().is_some() || self.jobs.outputting() {
            return;
        }
        let Some((&id, job)) = self.jobs.active.first_key_value() else {
            return;
        };
        if job.stage != Stage::Ready {
            return;
        }
        let job = self.jobs.active.get_mut(&id).unwrap();
        let text = job.result.as_ref().unwrap().text.as_ref().unwrap().clone();
        let config = job.config.clone();
        let generation = job.generation;
        let cancelled = job.cancelled.clone();
        job.transition(Stage::Delivering);
        let events = self.server.send.clone();
        let current = self.generation.clone();
        thread::spawn(move || {
            let result = if current.load(Ordering::Acquire) == generation {
                let resolved = output::resolve(config.string("output", "method"));
                output::deliver(&text, &config, &resolved, || {
                    cancelled.load(Ordering::Acquire)
                        || current.load(Ordering::Acquire) != generation
                })
            } else {
                Ok(())
            };
            let _ = events.send(Event::Delivered(id, result));
        });
    }
    fn end_key_capture(&mut self) {
        if let Some(capture) = self.key_capture.take() {
            self.jobs.listening = capture.restore;
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
                thread::spawn(move || {
                    let _ = thread.join();
                });
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
                    json!({
                    "type":"microphone_test","level":level,"message":message,"done":done}
                    ),
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
        self.worker_shutdown.store(true, Ordering::Release);
        self.benchmark_cancel.store(true, Ordering::Relaxed);
        self.jobs.generation += 1;
        self.generation
            .store(self.jobs.generation, Ordering::Release);
        let jobs = std::mem::take(&mut self.jobs.active);
        thread::spawn(move || drop(jobs));
        self.stop_test();
    }
}
fn retry_config(saved: &Config, message: &Value) -> Result<Config> {
    let mut changes = json!({});
    for key in ["transcription", "performance"] {
        if let Some(value) = message.get(key) {
            changes[key] = value.clone();
        }
    }
    let selected = saved.changed(&changes)?;
    // The inference supervisor applies the same CPU fallback as live dictation.
    // Validate user values in changed(), but do not reject a saved GPU preference
    // merely because this executable lacks that backend.
    if let Err(error) = optimization::check_profile(&selected) {
        if !matches!(
            optimization::profile(&selected),
            "vulkan" | "vulkan-full" | "hybrid"
        ) {
            return Err(error);
        }
        log::warn!(
            "History retry will use inference fallback: requested_profile={} reason={error:#}",
            optimization::profile(&selected)
        );
    }
    Ok(selected)
}
fn prepare_config(previous: &Config, message: &Value) -> Result<Config> {
    let old_model = previous.string("transcription", "model").to_owned();
    let old_setting = optimization::current_setting(previous);
    let mut candidate = if message["cmd"] == "reload_config" {
        Config::load()?
    } else if message["cmd"] == "set_log_level" {
        previous.changed(&json!({
        "logging":{
        "level":message["value"]}
        }
        ))?
    } else {
        previous.changed(&message["config"])?
    };
    let selected_model = candidate.string("transcription", "model").to_owned();
    if selected_model != old_model {
        let submitted_old_setting = message["config"]["performance"]["by_model"]
            .get(&old_model)
            .is_some_and(Value::is_object);
        if !submitted_old_setting {
            optimization::remember(&mut candidate, &old_model, old_setting);
        }
        if message["remember_performance"] == true {
            optimization::remember_current(&mut candidate);
        } else {
            optimization::activate_saved(&mut candidate);
        }
    } else if message["remember_performance"] == true {
        optimization::remember_current(&mut candidate);
    } else if message["cmd"] == "reload_config" {
        optimization::activate_saved(&mut candidate);
    }
    optimization::check_profile(&candidate)?;
    Ok(candidate)
}
fn open_path(path: &std::path::Path) -> Result<()> {
    let mut child = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
