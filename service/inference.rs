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

pub struct Inference {
    child: Child,
    socket: UnixStream,
    pending: Vec<u8>,
    identity: (String, String),
    pub profile: String,
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
        write_frame(&mut self.socket, request).context("Sending inference request")
    }
    fn response(
        &mut self,
        timeout: Duration,
        cancelled: &impl Fn() -> bool,
        preview: &mut impl FnMut(String, String),
    ) -> Result<Response> {
        let deadline = Instant::now() + timeout;
        loop {
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
                        Response::Failed { error } => bail!("{error}"),
                        response => return Ok(response),
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
