//! The only production boundary allowed to load native speech engines.
//! A persistent child owns the model; the supervisor can kill a stuck native call.
use crate::{
    config::Config,
    engine::{Engine, StreamInput},
    optimization, process,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 2 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Request {
    Load {
        config: Config,
    },
    Transcribe {
        path: std::path::PathBuf,
        config: Config,
    },
    Stream,
    Audio {
        samples: Vec<f32>,
    },
    Finish,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Response {
    Ready {
        profile: String,
    },
    Ack,
    Preview {
        committed: String,
        tentative: String,
    },
    Complete {
        text: Option<String>,
    },
    Failed {
        error: String,
    },
}
fn write_frame(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    anyhow::ensure!(data.len() <= MAX_FRAME, "Inference frame exceeds limit");
    stream.write_all(&(data.len() as u32).to_le_bytes())?;
    stream.write_all(&data)?;
    Ok(())
}
fn read_request(stream: &mut UnixStream) -> Result<Request> {
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    anyhow::ensure!(size <= MAX_FRAME, "Inference request exceeds limit");
    let mut data = vec![0; size];
    stream.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}

// Read only on a stalled/failed operation, never on the audio producer or UI thread.
fn worker_snapshot(pid: u32) -> serde_json::Value {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let fields: Vec<_> = stat
        .rsplit_once(") ")
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
    serde_json::json!({
        "state":fields.first(),
        "user_cpu_ticks":fields.get(11).and_then(|v| v.parse::<u64>().ok()),
        "system_cpu_ticks":fields.get(12).and_then(|v| v.parse::<u64>().ok()),
        "threads":fields.get(17).and_then(|v| v.parse::<u64>().ok()),
        "resident_pages":fields.get(21).and_then(|v| v.parse::<u64>().ok()),
        "wait_channel":std::fs::read_to_string(format!("/proc/{pid}/wchan")).ok().map(|v| v.trim().to_owned()),
        "host_load":std::fs::read_to_string("/proc/loadavg").ok().map(|v| v.trim().to_owned())
    })
}

pub struct Inference {
    child: Child,
    socket: UnixStream,
    pending: Vec<u8>,
    identity: (String, String),
    pub profile: String,
    operation: &'static str,
    sequence: u64,
    audio_samples: u64,
    operation_started: Instant,
    last_ack_samples: u64,
    max_response_ms: u128,
}
impl Inference {
    pub fn load(config: &Config, cancelled: impl Fn() -> bool) -> Result<Self> {
        Self::load_with_timeout(config, cancelled, LOAD_TIMEOUT)
    }
    fn load_with_timeout(
        config: &Config,
        cancelled: impl Fn() -> bool,
        timeout: Duration,
    ) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let mut command = Command::new(executable);
        #[cfg(not(test))]
        command.arg("--inference-worker");
        #[cfg(test)]
        command
            .args(["--exact", "inference::tests::worker_entry", "--nocapture"])
            .env("VOICE_TEST_INFERENCE_CHILD", "1");
        let (parent, child) = UnixStream::pair()?;
        command.env("VOICE_DICTATION_SESSION", crate::logging::session());
        process::kill_with_parent(&mut command);
        let mut child = command
            .stdin(Stdio::from(OwnedFd::from(child)))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(stderr) = child.stderr.take() {
            let pid = child.id();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut line = Vec::new();
                    match reader.by_ref().take(16384).read_until(b'\n', &mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            log::info!(target:"voice_dictation::native_stderr","worker_pid={pid} stderr={}",String::from_utf8_lossy(&line).trim_end())
                        }
                        Err(error) => {
                            log::warn!("Native stderr read failed: worker_pid={pid} error={error}");
                            break;
                        }
                    }
                }
            });
        }
        drop(command); // Close the parent copy of the child socket endpoint.
        parent.set_read_timeout(Some(Duration::from_millis(25)))?;
        parent.set_write_timeout(Some(Duration::from_millis(250)))?;
        log::info!(
            "Inference worker started: pid={} model={}",
            child.id(),
            config.string("transcription", "model")
        );
        let mut worker = Self {
            child,
            socket: parent,
            pending: Vec::new(),
            identity: optimization::identity(config),
            profile: String::new(),
            operation: "load",
            sequence: 0,
            audio_samples: 0,
            operation_started: Instant::now(),
            last_ack_samples: 0,
            max_response_ms: 0,
        };
        worker.send(&Request::Load {
            config: config.clone(),
        })?;
        match worker.response(timeout, &cancelled, &mut |_, _| {})? {
            Response::Ready { profile } => worker.profile = profile,
            response => bail!("Unexpected inference load response: {response:?}"),
        }
        Ok(worker)
    }
    pub fn matches(&self, c: &Config) -> bool {
        self.identity == optimization::identity(c)
    }
    fn send(&mut self, request: &Request) -> Result<()> {
        self.operation = match request {
            Request::Load { .. } => "load",
            Request::Transcribe { .. } => "transcribe",
            Request::Stream => {
                self.audio_samples = 0;
                self.last_ack_samples = 0;
                self.max_response_ms = 0;
                "stream_start"
            }
            Request::Audio { samples } => {
                self.audio_samples += samples.len() as u64;
                "stream_feed"
            }
            Request::Finish => "stream_finalize",
        };
        if self.operation == "stream_start" {
            log::info!(
                "Inference stream started: worker_pid={} runtime_profile={} worker={}",
                self.child.id(),
                self.profile,
                worker_snapshot(self.child.id())
            );
        }
        self.sequence += 1;
        self.operation_started = Instant::now();
        write_frame(&mut self.socket, request).with_context(|| {
            format!(
            "Sending inference request: worker_pid={} operation={} sequence={} audio_samples={}",
            self.child.id(), self.operation, self.sequence, self.audio_samples
        )
        })
    }
    fn response(
        &mut self,
        timeout: Duration,
        cancelled: &impl Fn() -> bool,
        preview: &mut impl FnMut(String, String),
    ) -> Result<Response> {
        let deadline = Instant::now() + timeout;
        let mut next_report = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() >= next_report || cancelled() || Instant::now() >= deadline {
                let outcome = if cancelled() {
                    "cancelled"
                } else if Instant::now() >= deadline {
                    "timeout"
                } else {
                    "waiting"
                };
                let mut details = crate::logging::current_context();
                if !details.is_object() {
                    details = serde_json::json!({});
                }
                let diagnostic = serde_json::json!({
                    "event":"inference_wait",
                    "worker_pid":self.child.id(), "operation":self.operation,
                    "sequence":self.sequence, "audio_samples":self.audio_samples,
                    "acknowledged_samples":self.last_ack_samples, "max_response_ms":self.max_response_ms,
                    "elapsed_ms":self.operation_started.elapsed().as_millis(),
                    "timeout_ms":timeout.as_millis(), "outcome":outcome,
                    "worker":worker_snapshot(self.child.id())
                });
                details
                    .as_object_mut()
                    .unwrap()
                    .extend(diagnostic.as_object().unwrap().clone());
                let _context = crate::logging::context(details);
                log::warn!("Inference {outcome}: worker_pid={} operation={} sequence={} audio_seconds={:.3} elapsed_ms={}",self.child.id(),self.operation,self.sequence,self.audio_samples as f64 / 16000.,self.operation_started.elapsed().as_millis());
                next_report = Instant::now() + Duration::from_secs(5);
            }
            anyhow::ensure!(!cancelled(), "Inference cancelled");
            anyhow::ensure!(
                Instant::now() < deadline,
                "Inference worker timed out after {}s",
                timeout.as_secs()
            );
            if self.pending.len() >= 4 {
                let size = u32::from_le_bytes(self.pending[..4].try_into().unwrap()) as usize;
                anyhow::ensure!(size <= MAX_FRAME, "Inference response exceeds limit");
                if self.pending.len() >= size + 4 {
                    let response: Response = serde_json::from_slice(&self.pending[4..size + 4])?;
                    self.pending.drain(..size + 4);
                    match response {
                        Response::Preview {
                            committed,
                            tentative,
                        } => {
                            preview(committed, tentative);
                            continue;
                        }
                        Response::Failed { error } => bail!("Native inference failed: worker_pid={} operation={} sequence={} audio_samples={} elapsed_ms={} error={error}",self.child.id(),self.operation,self.sequence,self.audio_samples,self.operation_started.elapsed().as_millis()),
                        response => {
                            self.max_response_ms = self.max_response_ms.max(self.operation_started.elapsed().as_millis());
                            if matches!(response, Response::Ack) && self.operation == "stream_feed" {
                                self.last_ack_samples = self.audio_samples;
                            }
                            if self.operation != "stream_feed"
                                || self.operation_started.elapsed() >= Duration::from_millis(500)
                            {
                                log::info!("Inference operation completed: worker_pid={} operation={} sequence={} audio_samples={} elapsed_ms={}", self.child.id(), self.operation, self.sequence, self.audio_samples, self.operation_started.elapsed().as_millis());
                            }
                            return Ok(response);
                        }
                    }
                }
            }
            let mut block = [0; 16384];
            match self.socket.read(&mut block) {
                Ok(0) => bail!(
                    "Inference worker disconnected (status={:?})",
                    self.child.try_wait()?
                ),
                Ok(n) => self.pending.extend_from_slice(&block[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e).context("Reading inference response"),
            }
        }
    }
    pub fn transcribe(
        &mut self,
        path: std::path::PathBuf,
        config: &Config,
        cancelled: impl Fn() -> bool,
    ) -> Result<String> {
        self.send(&Request::Transcribe {
            path,
            config: config.clone(),
        })?;
        match self.response(CALL_TIMEOUT, &cancelled, &mut |_, _| {})? {
            Response::Complete { text: Some(text) } => Ok(text),
            response => bail!("Unexpected transcription response: {response:?}"),
        }
    }
    pub fn stream(
        &mut self,
        receive: &mpsc::Receiver<StreamInput>,
        cancelled: impl Fn() -> bool,
        mut preview: impl FnMut(String, String),
    ) -> Result<Option<String>> {
        self.send(&Request::Stream)?;
        anyhow::ensure!(
            matches!(
                self.response(CALL_TIMEOUT, &cancelled, &mut preview)?,
                Response::Ack
            ),
            "Expected stream acknowledgement"
        );
        loop {
            anyhow::ensure!(!cancelled(), "Inference cancelled");
            let input = match receive.recv_timeout(Duration::from_millis(25)) {
                Ok(input) => input,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    anyhow::ensure!(
                        self.child.try_wait()?.is_none(),
                        "Inference worker exited during recording"
                    );
                    continue;
                }
                Err(_) => return Ok(None),
            };
            match input {
                StreamInput::Audio(samples) => {
                    self.send(&Request::Audio { samples })?;
                    anyhow::ensure!(
                        matches!(
                            self.response(CALL_TIMEOUT, &cancelled, &mut preview)?,
                            Response::Ack
                        ),
                        "Expected audio acknowledgement"
                    );
                }
                StreamInput::Finish => {
                    self.send(&Request::Finish)?;
                    return match self.response(CALL_TIMEOUT, &cancelled, &mut preview)? {
                        Response::Complete { text } => Ok(text),
                        response => bail!("Unexpected stream result: {response:?}"),
                    };
                }
                StreamInput::Cancel => return Ok(None),
            }
        }
    }
}
impl Drop for Inference {
    fn drop(&mut self) {
        log::info!("Inference worker stopping: pid={}", self.child.id());
        let already_exited = self.child.try_wait().ok().flatten();
        if already_exited.is_none() {
            let _ = self.child.kill();
        }
        match self.child.wait() {
            Ok(status) => {
                use std::os::unix::process::ExitStatusExt;
                log::info!("Inference worker reaped: worker_pid={} code={:?} signal={:?} core_dumped={} supervisor_kill={}",self.child.id(),status.code(),status.signal(),status.core_dumped(),already_exited.is_none());
            }
            Err(error) => log::error!(
                "Inference worker reap failed: worker_pid={} error={error}",
                self.child.id()
            ),
        }
    }
}

/// Private child entry point. stdin is a duplex Unix socket, not user input.
pub fn child() -> Result<()> {
    // SAFETY: the supervisor passes ownership of its socket as fd 0.
    let mut socket = unsafe { UnixStream::from_raw_fd(0) };
    let mut engine: Option<Engine> = None;
    #[cfg(test)]
    let mut synthetic = false;
    loop {
        let request = read_request(&mut socket)?;
        #[cfg(test)]
        {
            if let Request::Load { config } = &request {
                match config.string("transcription", "model") {
                    "test-crash" => std::process::exit(17),
                    "test-stall" => std::thread::sleep(Duration::from_secs(60)),
                    "test-echo" => {
                        synthetic = true;
                        write_frame(
                            &mut socket,
                            &Response::Ready {
                                profile: "synthetic".into(),
                            },
                        )?;
                        continue;
                    }
                    _ => {}
                }
            }
            if synthetic {
                let response = match request {
                    Request::Transcribe { .. } => Response::Complete {
                        text: Some("synthetic transcript".into()),
                    },
                    Request::Finish => Response::Complete {
                        text: Some("synthetic streaming transcript".into()),
                    },
                    _ => Response::Ack,
                };
                write_frame(&mut socket, &response)?;
                continue;
            }
        }
        let result = (|| -> Result<Response> {
            match request {
                Request::Load { config } => {
                    engine = None;
                    let mut profile = optimization::profile(&config).to_owned();
                    let loaded = Engine::load(&config).or_else(|error| {
                        if !matches!(
                            optimization::profile(&config),
                            "vulkan" | "vulkan-full" | "hybrid"
                        ) {
                            return Err(error);
                        }
                        profile = format!("CPU fallback: {error:#}");
                        let cpu = config
                            .changed(&serde_json::json!({"performance":{"profile":"standard"}}))?;
                        Engine::load(&cpu)
                    })?;
                    engine = Some(loaded);
                    Ok(Response::Ready { profile })
                }
                Request::Transcribe { path, config } => {
                    let samples =
                        crate::engine::read_audio(&path, config.flag("audio", "preprocess"))?;
                    let text = engine
                        .as_mut()
                        .context("No model loaded")?
                        .transcribe(&samples, &config)?;
                    Ok(Response::Complete { text: Some(text) })
                }
                Request::Stream => {
                    write_frame(&mut socket, &Response::Ack)?;
                    let writer = std::cell::RefCell::new(socket.try_clone()?);
                    let mut acknowledge = false;
                    let text = engine.as_mut().context("No model loaded")?.stream_inputs(
                        || {
                            if acknowledge {
                                write_frame(&mut writer.borrow_mut(), &Response::Ack)?;
                            }
                            match read_request(&mut socket)? {
                                Request::Audio { samples } => {
                                    acknowledge = true;
                                    Ok(Some(StreamInput::Audio(samples)))
                                }
                                Request::Finish => Ok(Some(StreamInput::Finish)),
                                _ => bail!("Unexpected input during stream"),
                            }
                        },
                        |committed, tentative| {
                            let _ = write_frame(
                                &mut writer.borrow_mut(),
                                &Response::Preview {
                                    committed,
                                    tentative,
                                },
                            );
                        },
                    )?;
                    Ok(Response::Complete { text })
                }
                _ => bail!("No active stream"),
            }
        })();
        match result {
            Ok(response) => write_frame(&mut socket, &response)?,
            Err(error) => {
                write_frame(
                    &mut socket,
                    &Response::Failed {
                        error: format!("{error:#}"),
                    },
                )?;
                // A failed native session is never reused.
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_snapshot_is_readable_and_missing_process_is_tolerated() {
        let snapshot = worker_snapshot(std::process::id());
        assert!(snapshot["threads"].as_u64().unwrap() > 0);
        assert!(snapshot["resident_pages"].as_u64().unwrap() > 0);
        assert!(worker_snapshot(u32::MAX)["state"].is_null());
    }

    #[test]
    fn worker_entry() {
        if std::env::var_os("VOICE_TEST_INFERENCE_CHILD").is_some() {
            let _ = child();
        }
    }
    fn config(model: &str) -> (tempfile::TempDir, Config) {
        let root = tempfile::tempdir().unwrap();
        let config = Config::at(root.path())
            .unwrap()
            .changed(&serde_json::json!({"transcription":{"model":model}}))
            .unwrap();
        (root, config)
    }
    #[test]
    fn crashed_worker_returns_error_and_next_worker_can_start() {
        let (_root, c) = config("test-crash");
        let started = Instant::now();
        assert!(Inference::load(&c, || false)
            .err()
            .unwrap()
            .to_string()
            .contains("disconnected"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let (_root, c) = config("/nonexistent/synthetic-model.gguf");
        assert!(Inference::load(&c, || false)
            .err()
            .unwrap()
            .to_string()
            .contains("does not exist"));
    }
    #[test]
    fn cancellation_terminates_stalled_native_worker() {
        let (_root, c) = config("test-stall");
        let started = Instant::now();
        let error = Inference::load(&c, || started.elapsed() > Duration::from_millis(150))
            .err()
            .unwrap();
        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn stream_and_batch_share_one_resident_worker() {
        let (_root, c) = config("test-echo");
        let mut worker = Inference::load(&c, || false).unwrap();
        let pid = worker.child.id();
        let (send, receive) = mpsc::channel();
        send.send(StreamInput::Audio(vec![0.; 1600])).unwrap();
        send.send(StreamInput::Finish).unwrap();
        assert_eq!(
            worker
                .stream(&receive, || false, |_, _| {})
                .unwrap()
                .as_deref(),
            Some("synthetic streaming transcript")
        );
        assert_eq!(
            worker
                .transcribe("synthetic.wav".into(), &c, || false)
                .unwrap(),
            "synthetic transcript"
        );
        assert_eq!(worker.child.id(), pid);
        assert_eq!(worker.audio_samples, 1600);
        assert_eq!(worker.last_ack_samples, 1600);
        assert_eq!(worker.operation, "transcribe");
    }
    #[test]
    fn load_timeout_reaps_stalled_worker() {
        let (_root, c) = config("test-stall");
        let started = Instant::now();
        let error = Inference::load_with_timeout(&c, || false, Duration::from_millis(150))
            .err()
            .unwrap();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
