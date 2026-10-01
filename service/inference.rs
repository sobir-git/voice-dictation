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
// Capture can deliver a large callback after a scheduling pause. Keep each JSON
// audio frame comfortably below MAX_FRAME without bounding the capture queue.
const STREAM_IPC_MAX_SAMPLES: usize = 16_000;
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
fn idle_warmup_due(elapsed: Duration) -> bool {
    elapsed >= Duration::from_secs(60)
}

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
    IdleWarmup,
    Stream,
    Audio {
        samples: Vec<f32>,
    },
    Finish,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Response {
    LoadDiagnostic {
        details: serde_json::Value,
    },
    Timing {
        details: serde_json::Value,
    },
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
        "memory":crate::paging::Snapshot::read_process(pid),
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
    ready_at: Instant,
    last_attempt: Option<Instant>,
    last_completed: Instant,
    inference_attempts: u64,
    #[cfg(test)]
    timing_records: Vec<serde_json::Value>,
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
            ready_at: Instant::now(),
            last_completed: Instant::now(),
            last_attempt: None,
            inference_attempts: 0,
            #[cfg(test)]
            timing_records: Vec::new(),
        };
        worker.send(&Request::Load {
            config: config.clone(),
        })?;
        match worker.response(timeout, &cancelled, &mut |_, _| {})? {
            Response::Ready { profile } => worker.profile = profile,
            response => bail!("Unexpected inference load response: {response:?}"),
        }
        worker.ready_at = Instant::now();
        worker.last_completed = worker.ready_at;
        Ok(worker)
    }
    pub fn matches(&self, c: &Config) -> bool {
        self.identity == optimization::identity(c)
    }
    fn send(&mut self, request: &Request) -> Result<()> {
        if matches!(request, Request::Stream | Request::Transcribe { .. }) {
            self.inference_attempts += 1;
            let mut details = crate::logging::current_context();
            details["event"] = "inference_attempt".into();
            details["worker_pid"] = self.child.id().into();
            details["attempt"] = self.inference_attempts.into();
            details["first_after_load"] = (self.inference_attempts == 1).into();
            details["model_ready_age_ms"] = (self.ready_at.elapsed().as_secs_f64() * 1000.).into();
            details["since_previous_attempt_ms"] =
                serde_json::json!(self.last_attempt.map(|t| t.elapsed().as_secs_f64() * 1000.));
            let _context = crate::logging::context(details);
            log::info!("Inference attempt started");
            self.last_attempt = Some(Instant::now());
        }
        self.operation = match request {
            Request::Load { .. } => "load",
            Request::IdleWarmup => "idle_warmup",
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
        log::debug!(
            "Inference operation started: worker_pid={} operation={} sequence={} audio_samples={}",
            self.child.id(),
            self.operation,
            self.sequence,
            self.audio_samples
        );
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
        let mut deadline = Instant::now() + timeout;
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
                        Response::LoadDiagnostic { details } | Response::Timing { details } => {
                            let mut context = crate::logging::current_context();
                            if !context.is_object() { context = serde_json::json!({}); }
                            context.as_object_mut().unwrap().extend(details.as_object().context("Invalid timing diagnostic")?.clone());
                            context["worker_pid"] = self.child.id().into();
                            // This is the receiving IPC operation, not a native call identifier.
                            context["receiving_operation"] = self.operation.into();
                            context["receiving_sequence"] = self.sequence.into();
                            context["supervisor_elapsed_ms"] = (self.operation_started.elapsed().as_secs_f64()*1000.).into();
                            #[cfg(test)]
                            self.timing_records.push(context.clone());
                            let _context = crate::logging::context(context);
                            log::info!("Inference timing diagnostic");
                            if matches!(self.operation, "stream_feed" | "stream_finalize") { deadline = Instant::now() + timeout; }
                            continue;
                        }
                        Response::Preview {
                            committed,
                            tentative,
                        } => {
                            if matches!(self.operation, "stream_feed" | "stream_finalize") { deadline = Instant::now() + timeout; }
                            preview(committed, tentative);
                            continue;
                        }
                        Response::Failed { error } => bail!("Native inference failed: worker_pid={} operation={} sequence={} audio_samples={} elapsed_ms={} error={error}",self.child.id(),self.operation,self.sequence,self.audio_samples,self.operation_started.elapsed().as_millis()),
                        response => {
                            if matches!(response, Response::Complete { text: Some(_) })
                                || (matches!(response, Response::Ack) && self.operation == "idle_warmup")
                            {
                                self.last_completed = Instant::now();
                            }
                            self.max_response_ms = self.max_response_ms.max(self.operation_started.elapsed().as_millis());
                            if matches!(response, Response::Ack) && self.operation == "stream_feed" {
                                self.last_ack_samples = self.audio_samples;
                            }
                            if self.operation != "stream_feed"
                                || self.operation_started.elapsed() >= Duration::from_millis(10)
                            {
                                log::info!("Inference operation completed: worker_pid={} operation={} sequence={} audio_samples={} elapsed_ms={}", self.child.id(), self.operation, self.sequence, self.audio_samples, self.operation_started.elapsed().as_millis());
                            } else {
                                log::debug!("Inference operation completed: worker_pid={} operation={} sequence={} audio_samples={} elapsed_ms={:.3}", self.child.id(), self.operation, self.sequence, self.audio_samples, self.operation_started.elapsed().as_secs_f64()*1000.);
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
    pub fn warm_if_idle(&mut self, cancelled: impl Fn() -> bool) -> Result<()> {
        anyhow::ensure!(!cancelled(), "Inference cancelled");
        let elapsed = self.last_completed.elapsed();
        let needed = idle_warmup_due(elapsed);
        let mut details = crate::logging::current_context();
        details["event"] = "idle_warmup_decision".into();
        details["worker_pid"] = self.child.id().into();
        details["idle_since_completion_ms"] = (elapsed.as_secs_f64() * 1000.).into();
        details["threshold_ms"] = 60_000.into();
        details["needed"] = needed.into();
        details["decision"] = if needed { "needed" } else { "skipped" }.into();
        #[cfg(test)]
        self.timing_records.push(details.clone());
        {
            let _context = crate::logging::context(details);
            log::info!("Recording admission warmup decision");
        }
        if needed {
            self.send(&Request::IdleWarmup)?;
            anyhow::ensure!(
                matches!(
                    self.response(CALL_TIMEOUT, &cancelled, &mut |_, _| {})?,
                    Response::Ack
                ),
                "Expected idle warmup acknowledgement"
            );
        }
        Ok(())
    }
    pub fn stream(
        &mut self,
        receive: &crate::stream_queue::Receiver,
        cancelled: impl Fn() -> bool,
        mut preview: impl FnMut(String, String),
    ) -> Result<Option<String>> {
        let started = Instant::now();
        let mut feed_calls = 0_u64;
        let mut max_queue_ms = 0_f64;
        let mut max_call_ms = 0_f64;
        let mut lag_warned = false;
        let mut recent_real_time_factor = 0_f64;
        let mut max_backlog_seconds = 0_f64;
        let first_preview_ms = std::cell::Cell::new(None);
        let worker_pid = self.child.id();
        let mut preview_timed = |committed: String, tentative: String| {
            if first_preview_ms.get().is_none() && (!committed.is_empty() || !tentative.is_empty())
            {
                let elapsed = started.elapsed().as_secs_f64() * 1000.;
                first_preview_ms.set(Some(elapsed));
                let mut details = crate::logging::current_context();
                details["event"] = "first_preview".into();
                details["worker_pid"] = worker_pid.into();
                details["stream_elapsed_ms"] = elapsed.into();
                let _context = crate::logging::context(details);
                log::info!("First transcription preview");
            }
            preview(committed, tentative);
        };
        let result = (|| {
            self.warm_if_idle(&cancelled)?;
            self.send(&Request::Stream)?;
            anyhow::ensure!(
                matches!(
                    self.response(CALL_TIMEOUT, &cancelled, &mut preview_timed)?,
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
                    StreamInput::Audio { samples, queued } => {
                        let backlog_seconds = receive.backlog_seconds();
                        max_backlog_seconds = max_backlog_seconds.max(backlog_seconds);
                        // Chunking only at IPC keeps a delayed capture callback from
                        // exceeding the wire limit; native buffered feeds retain order.
                        for chunk in samples.chunks(STREAM_IPC_MAX_SAMPLES) {
                            let sample_count = chunk.len();
                            let queue_ms = queued.elapsed().as_secs_f64() * 1000.;
                            max_queue_ms = max_queue_ms.max(queue_ms);
                            feed_calls += 1;
                            let call_started = Instant::now();
                            let response = self
                                .send(&Request::Audio {
                                    samples: chunk.to_vec(),
                                })
                                .and_then(|()| {
                                    self.response(CALL_TIMEOUT, &cancelled, &mut preview_timed)
                                });
                            let call_ms = call_started.elapsed().as_secs_f64() * 1000.;
                            max_call_ms = max_call_ms.max(call_ms);
                            recent_real_time_factor = call_ms * 16. / sample_count as f64;
                            let backlog_seconds = receive.backlog_seconds();
                            max_backlog_seconds = max_backlog_seconds.max(backlog_seconds);
                            if !lag_warned
                                && backlog_seconds > crate::stream_queue::STREAM_LAG_WARNING_SECONDS
                            {
                                lag_warned = true;
                                let mut details = crate::logging::current_context();
                                details["event"] = "stream_lagging".into();
                                details["backlog_seconds"] = backlog_seconds.into();
                                details["recent_real_time_factor"] = recent_real_time_factor.into();
                                #[cfg(test)]
                                self.timing_records.push(details.clone());
                                let _context = crate::logging::context(details);
                                log::warn!("Streaming inference is falling behind capture");
                            }
                            let significant =
                                call_ms >= 10. || queue_ms >= 100. || response.is_err();
                            if significant || log::log_enabled!(log::Level::Debug) || cfg!(test) {
                                let mut details = crate::logging::current_context();
                                details["event"] = "audio_queue_timing".into();
                                details["worker_pid"] = worker_pid.into();
                                details["feed_call"] = feed_calls.into();
                                details["audio_samples"] = self.audio_samples.into();
                                details["queue_ms"] = queue_ms.into();
                                details["call_ms"] = call_ms.into();
                                details["success"] = response.is_ok().into();
                                {
                                    #[cfg(test)]
                                    self.timing_records.push(details.clone());
                                    let _context = crate::logging::context(details);
                                    if significant {
                                        log::info!("Audio queue timing");
                                    } else {
                                        log::debug!("Audio queue timing");
                                    }
                                }
                            }
                            anyhow::ensure!(
                                matches!(response?, Response::Ack),
                                "Expected audio acknowledgement"
                            );
                            receive.acknowledge(sample_count);
                        }
                    }
                    StreamInput::Finish => {
                        self.send(&Request::Finish)?;
                        return match self.response(CALL_TIMEOUT, &cancelled, &mut preview_timed)? {
                            Response::Complete { text } => Ok(text),
                            response => bail!("Unexpected stream result: {response:?}"),
                        };
                    }
                    StreamInput::Cancel => return Ok(None),
                }
            }
        })();
        let mut details = crate::logging::current_context();
        details["event"] = "stream_timing_summary".into();
        details["worker_pid"] = worker_pid.into();
        details["feed_calls"] = feed_calls.into();
        details["audio_samples"] = self.audio_samples.into();
        details["max_queue_ms"] = max_queue_ms.into();
        details["max_call_ms"] = max_call_ms.into();
        details["max_backlog_seconds"] = max_backlog_seconds.into();
        details["final_backlog_seconds"] = receive.backlog_seconds().into();
        details["backlog_at_finish_seconds"] = serde_json::json!(receive.finish().map(|(_, n)| n));
        details["catch_up_ms"] = serde_json::json!(receive
            .finish()
            .map(|(t, _)| t.elapsed().as_secs_f64() * 1000.));
        details["recent_real_time_factor"] = recent_real_time_factor.into();
        details["first_preview_ms"] = serde_json::json!(first_preview_ms.get());
        details["stream_elapsed_ms"] = (started.elapsed().as_secs_f64() * 1000.).into();
        details["outcome"] = match &result {
            Ok(Some(_)) => "completed",
            Ok(None) => "cancelled",
            Err(_) if cancelled() => "cancelled",
            Err(_) => "failed",
        }
        .into();
        #[cfg(test)]
        self.timing_records.push(details.clone());
        let _context = crate::logging::context(details);
        log::info!("Stream timing summary");
        result
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

fn load_with_fallback<T>(
    config: &Config,
    mut load: impl FnMut(&Config) -> Result<T>,
    gpu_attempted: impl Fn() -> bool,
    mut emit: impl FnMut(serde_json::Value) -> Result<()>,
) -> Result<(T, String)> {
    let started = Instant::now();
    let requested = optimization::profile(config);
    let gpu = matches!(requested, "vulkan" | "vulkan-full" | "hybrid");
    let diagnostic = |stage: &str,
                      reason: &str,
                      attempted: bool,
                      actual: &str,
                      error: Option<String>| {
        serde_json::json!({"event":"model_load", "stage":stage,"reason":reason,"elapsed_ms":started.elapsed().as_millis(),
            "model":config.string("transcription","model"),"requested_profile":requested,
            "compiled_backend":if cfg!(feature="vulkan") {"vulkan"} else {"cpu"},
            "gpu_attempted":attempted,"actual_profile":actual,"error":error})
    };
    emit(diagnostic("attempt", "requested", false, "pending", None))?;
    let validation = optimization::check_profile(config);
    let preflight_failed = validation.is_err();
    let result = validation.and_then(|_| load(config));
    match result {
        Ok(engine) => {
            emit(diagnostic(
                "success",
                "requested_loaded",
                gpu_attempted(),
                requested,
                None,
            ))?;
            Ok((engine, requested.into()))
        }
        Err(error) => {
            let attempted = !preflight_failed && gpu_attempted();
            let reason = if gpu && !cfg!(feature = "vulkan") {
                "backend_not_compiled"
            } else if preflight_failed {
                "profile_validation_failed"
            } else if attempted {
                "gpu_initialization_or_model_load_failed"
            } else {
                "model_load_failed_before_gpu_init"
            };
            emit(diagnostic(
                if gpu { "fallback" } else { "failure" },
                reason,
                attempted,
                "pending",
                Some(format!("{error:#}")),
            ))?;
            if !gpu {
                return Err(error);
            }
            let cpu = config.changed(&serde_json::json!({"performance":{"profile":"standard"}}))?;
            let loaded = load(&cpu).inspect_err(|cpu_error| {
                let _ = emit(diagnostic(
                    "failure",
                    "cpu_fallback_failed",
                    attempted,
                    "none",
                    Some(format!("{cpu_error:#}")),
                ));
            })?;
            emit(diagnostic(
                "success",
                "cpu_fallback_loaded",
                attempted,
                "standard",
                None,
            ))?;
            Ok((loaded, format!("CPU fallback: {error:#}")))
        }
    }
}

/// Private child entry point. stdin is a duplex Unix socket, not user input.
pub fn child() -> Result<()> {
    // SAFETY: the supervisor passes ownership of its socket as fd 0.
    let mut socket = unsafe { UnixStream::from_raw_fd(0) };
    let mut engine: Option<Engine> = None;
    let mut loaded_config: Option<Config> = None;
    #[cfg(test)]
    let mut synthetic = None;
    loop {
        let request = read_request(&mut socket)?;
        #[cfg(test)]
        {
            if let Request::Load { config } = &request {
                match config.string("transcription", "model") {
                    "test-crash" => std::process::exit(17),
                    "test-stall" => std::thread::sleep(Duration::from_secs(60)),
                    mode @ ("test-echo"
                    | "test-slow-stream"
                    | "test-stream-progress"
                    | "test-warmup-fail"
                    | "test-warmup-stall") => {
                        synthetic = Some(mode.to_owned());
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
            if let Some(mode) = &synthetic {
                if mode == "test-stream-progress"
                    && matches!(request, Request::Audio { .. } | Request::Finish)
                {
                    for _ in 0..4 {
                        std::thread::sleep(Duration::from_millis(50));
                        write_frame(
                            &mut socket,
                            &Response::Timing {
                                details: serde_json::json!({"event":"native_timing", "stage":"synthetic_progress"}),
                            },
                        )?;
                    }
                }
                if mode == "test-slow-stream" && matches!(request, Request::Audio { .. }) {
                    std::thread::sleep(Duration::from_millis(16));
                }
                if matches!(request, Request::IdleWarmup) {
                    if mode == "test-warmup-stall" {
                        std::thread::sleep(Duration::from_secs(60));
                    }
                    let before = crate::paging::Snapshot::read();
                    write_frame(
                        &mut socket,
                        &Response::Timing {
                            details: before.record("idle_warmup", "before", None),
                        },
                    )?;
                    write_frame(
                        &mut socket,
                        &Response::Timing {
                            details: serde_json::json!({"event":"native_timing","stage":"idle_warmup","audio_samples":33280,"fixture":"silence"}),
                        },
                    )?;
                    write_frame(
                        &mut socket,
                        &Response::Timing {
                            details: crate::paging::Snapshot::read().record(
                                "idle_warmup",
                                "after",
                                Some(&before),
                            ),
                        },
                    )?;
                    write_frame(
                        &mut socket,
                        &if mode == "test-warmup-fail" {
                            Response::Failed {
                                error: "synthetic warmup failure".into(),
                            }
                        } else {
                            Response::Ack
                        },
                    )?;
                    continue;
                }
                write_frame(
                    &mut socket,
                    &Response::Timing {
                        details: serde_json::json!({"event":"native_timing", "stage":"synthetic", "wall_ms":0.25}),
                    },
                )?;
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
        let attempt_stage = match &request {
            Request::Transcribe { .. } => Some("inference_attempt"),
            Request::Stream => Some("stream_attempt"),
            _ => None,
        };
        let memory_before = attempt_stage.map(|stage| {
            let before = crate::paging::Snapshot::read();
            (stage, before)
        });
        if let Some((stage, before)) = &memory_before {
            write_frame(
                &mut socket,
                &Response::Timing {
                    details: before.record(stage, "before", None),
                },
            )?;
        }
        let result = (|| -> Result<Response> {
            match request {
                Request::Load { config } => {
                    engine = None;
                    let writer = std::cell::RefCell::new(socket.try_clone()?);
                    let (loaded, profile) = load_with_fallback(
                        &config,
                        |c| {
                            let loaded = Engine::load_with_diagnostics(c, &mut |details| {
                                write_frame(&mut writer.borrow_mut(), &Response::Timing { details })
                            })?;
                            loaded_config = Some(c.clone());
                            Ok(loaded)
                        },
                        crate::engine::gpu_attempted,
                        |details| {
                            write_frame(
                                &mut writer.borrow_mut(),
                                &Response::LoadDiagnostic { details },
                            )
                        },
                    )?;
                    engine = Some(loaded);
                    Ok(Response::Ready { profile })
                }
                Request::IdleWarmup => {
                    engine.as_mut().context("No model loaded")?.idle_warmup(
                        loaded_config.as_ref().context("No loaded configuration")?,
                        &mut |details| write_frame(&mut socket, &Response::Timing { details }),
                    )?;
                    Ok(Response::Ack)
                }
                Request::Transcribe { path, config } => {
                    let started = Instant::now();
                    let samples =
                        crate::engine::read_audio(&path, config.flag("audio", "preprocess"))?;
                    write_frame(
                        &mut socket,
                        &Response::Timing {
                            details: serde_json::json!({"event":"native_timing", "stage":"audio_prepare", "wall_ms":started.elapsed().as_secs_f64()*1000., "audio_samples":samples.len()}),
                        },
                    )?;
                    let text = engine
                        .as_mut()
                        .context("No model loaded")?
                        .transcribe_with_diagnostics(&samples, &config, &mut |details| {
                            write_frame(&mut socket, &Response::Timing { details })
                        })?;
                    Ok(Response::Complete { text: Some(text) })
                }
                Request::Stream => {
                    let writer = std::cell::RefCell::new(socket.try_clone()?);
                    // Acknowledge stream start only after stream_begin telemetry. This
                    // keeps its initialization cost in the stream-start request.
                    let mut acknowledge = true;
                    let text = engine
                        .as_mut()
                        .context("No model loaded")?
                        .stream_inputs_with_diagnostics(
                            || {
                                if acknowledge {
                                    write_frame(&mut writer.borrow_mut(), &Response::Ack)?;
                                }
                                match read_request(&mut socket)? {
                                    Request::Audio { samples } => {
                                        acknowledge = true;
                                        Ok(Some(StreamInput::audio(samples)))
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
                            &mut |details| {
                                write_frame(&mut writer.borrow_mut(), &Response::Timing { details })
                            },
                        )?;
                    Ok(Response::Complete { text })
                }
                _ => bail!("No active stream"),
            }
        })();
        if let Some((stage, before)) = &memory_before {
            let mut details = crate::paging::Snapshot::read().record(stage, "after", Some(before));
            details["success"] = result.is_ok().into();
            write_frame(&mut socket, &Response::Timing { details })?;
        }
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
    #[test]
    fn fallback_diagnostic_distinguishes_compilation_from_gpu_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::at(dir.path())
            .unwrap()
            .changed(&serde_json::json!({"performance":{"profile":"vulkan"}}))
            .unwrap();
        let mut records = Vec::new();
        let ((), profile) = super::load_with_fallback(
            &config,
            |c| {
                if crate::optimization::profile(c) == "vulkan" {
                    anyhow::bail!("synthetic GPU failure");
                }
                Ok(())
            },
            || true,
            |v| {
                records.push(v);
                Ok(())
            },
        )
        .unwrap();
        assert!(profile.starts_with("CPU fallback:"));
        assert_eq!(
            records[1]["reason"],
            if cfg!(feature = "vulkan") {
                "gpu_initialization_or_model_load_failed"
            } else {
                "backend_not_compiled"
            }
        );
        assert_eq!(records[1]["gpu_attempted"], cfg!(feature = "vulkan"));
        assert_eq!(records.last().unwrap()["actual_profile"], "standard");
    }

    use super::*;
    #[test]
    fn worker_snapshot_is_readable_and_missing_process_is_tolerated() {
        let snapshot = worker_snapshot(std::process::id());
        assert!(snapshot["threads"].as_u64().unwrap() > 0);
        assert!(snapshot["resident_pages"].as_u64().unwrap() > 0);
        assert!(snapshot["memory"]["vm_swap_kb"].is_number());
        assert!(snapshot["memory"]["minor_faults"].is_number());
        assert!(snapshot["memory"]["major_faults"].is_number());
        let missing = worker_snapshot(u32::MAX);
        assert!(missing["state"].is_null());
        assert!(missing["memory"]["vm_swap_kb"].is_null());
        assert!(missing["memory"]["major_faults"].is_null());
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
    fn idle_boundary_and_completion_clock() {
        assert!(!idle_warmup_due(Duration::from_millis(59_999)));
        assert!(idle_warmup_due(Duration::from_secs(60)));
        let (_root, c) = config("test-echo");
        let mut worker = Inference::load(&c, || false).unwrap();
        worker.warm_if_idle(|| false).unwrap();
        assert_eq!(worker.sequence, 1, "fresh load is already warm");
        let decision = worker.timing_records.last().unwrap();
        assert_eq!(decision["event"], "idle_warmup_decision");
        assert_eq!(decision["decision"], "skipped");
        assert_eq!(decision["threshold_ms"], 60_000);
        assert!(decision["idle_since_completion_ms"].as_f64().unwrap() < 60_000.);
        let old = Instant::now() - Duration::from_secs(61);
        worker.last_completed = old;
        let pid = worker.child.id();
        let _context = crate::logging::context(serde_json::json!({"job_id":123}));
        // Producer queues real input while warmup runs; synthetic samples never enter this queue.
        let (send, receive) = crate::stream_queue::channel();
        let producer = std::thread::spawn(move || {
            send.send(StreamInput::audio(vec![0.25; 1600])).unwrap();
            send.send(StreamInput::Finish).unwrap();
        });
        assert_eq!(
            worker
                .stream(&receive, || false, |_, _| {})
                .unwrap()
                .as_deref(),
            Some("synthetic streaming transcript")
        );
        producer.join().unwrap();
        assert_eq!(worker.audio_samples, 1600);
        let decision = worker
            .timing_records
            .iter()
            .find(|r| r["event"] == "idle_warmup_decision" && r["needed"] == true)
            .unwrap();
        assert_eq!(decision["decision"], "needed");
        assert_eq!(decision["worker_pid"], pid);
        assert_eq!(decision["job_id"], 123);
        assert!(decision["idle_since_completion_ms"].as_f64().unwrap() >= 60_000.);
        assert_eq!(worker.inference_attempts, 1);
        assert_eq!(worker.child.id(), pid);
        let warm: Vec<_> = worker
            .timing_records
            .iter()
            .filter(|r| r["stage"] == "idle_warmup")
            .collect();
        assert_eq!(warm.len(), 3);
        assert!(warm
            .iter()
            .all(|r| r["receiving_operation"] == "idle_warmup"
                && r["worker_pid"] == pid
                && r["job_id"] == 123));
        assert_eq!(warm[0]["phase"], "before");
        assert_eq!(warm[2]["phase"], "after");
        assert!(warm[2]["minor_faults_delta"].is_number());
        assert!(worker.last_completed > old);
        let sequence = worker.sequence;
        // A long recording's start/attempt age must not make its completed engine cold.
        worker.last_attempt = Some(old);
        worker.warm_if_idle(|| false).unwrap();
        assert_eq!(worker.sequence, sequence);
        assert_eq!(worker.inference_attempts, 1);
        worker.send(&Request::Stream).unwrap();
        assert!(matches!(
            worker
                .response(CALL_TIMEOUT, &|| false, &mut |_, _| {})
                .unwrap(),
            Response::Ack
        ));
        worker.last_completed = old; // Simulate a recording spanning more than 60 seconds.
        worker.send(&Request::Finish).unwrap();
        assert!(matches!(
            worker
                .response(CALL_TIMEOUT, &|| false, &mut |_, _| {})
                .unwrap(),
            Response::Complete { .. }
        ));
        let sequence = worker.sequence;
        worker.warm_if_idle(|| false).unwrap();
        assert_eq!(
            worker.sequence, sequence,
            "long recording is warm from completion"
        );
    }
    #[test]
    fn failed_cancelled_and_timed_out_warmup_do_not_advance_clock() {
        for mode in ["test-warmup-fail", "test-warmup-stall"] {
            let (_root, c) = config(mode);
            let mut worker = Inference::load(&c, || false).unwrap();
            let old = Instant::now() - Duration::from_secs(61);
            worker.last_completed = old;
            assert!(worker.warm_if_idle(|| true).is_err());
            assert_eq!(worker.sequence, 1, "cancel before admission");
            worker.send(&Request::IdleWarmup).unwrap();
            let started = Instant::now();
            let error = worker
                .response(Duration::from_millis(150), &|| false, &mut |_, _| {})
                .unwrap_err();
            assert!(error.to_string().contains(if mode == "test-warmup-stall" {
                "timed out"
            } else {
                "synthetic warmup failure"
            }));
            assert_eq!(worker.last_completed, old);
            assert_eq!(worker.inference_attempts, 0);
            assert!(started.elapsed() < Duration::from_secs(3));
        }
        let (_root, c) = config("test-warmup-stall");
        let mut worker = Inference::load(&c, || false).unwrap();
        worker.last_completed = Instant::now() - Duration::from_secs(61);
        let old = worker.last_completed;
        let started = Instant::now();
        assert!(worker
            .warm_if_idle(|| started.elapsed() > Duration::from_millis(150))
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        assert_eq!(worker.last_completed, old);
    }
    #[test]
    fn stream_and_batch_share_one_resident_worker() {
        let (_root, c) = config("test-echo");
        let mut worker = Inference::load(&c, || false).unwrap();
        let pid = worker.child.id();
        let (send, receive) = crate::stream_queue::channel();
        send.send(StreamInput::audio(vec![0.; 1600])).unwrap();
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
        assert_eq!(worker.inference_attempts, 2);
        let summary = worker
            .timing_records
            .iter()
            .find(|r| r["event"] == "stream_timing_summary")
            .unwrap();
        assert_eq!(summary["outcome"], "completed");
        assert_eq!(summary["feed_calls"], 1);
        assert_eq!(summary["audio_samples"], 1600);
        assert!(worker
            .timing_records
            .iter()
            .any(|r| r["stage"] == "synthetic"
                && r["receiving_operation"] == "stream_start"
                && r["receiving_sequence"] == 2));
        assert!(worker
            .timing_records
            .iter()
            .any(|r| r["stage"] == "synthetic"
                && r["receiving_operation"] == "stream_feed"
                && r["worker_pid"] == pid));
    }
    #[test]
    fn streaming_timeout_tracks_inactivity_instead_of_total_catch_up() {
        let (_root, c) = config("test-stream-progress");
        let mut worker = Inference::load(&c, || false).unwrap();
        worker.send(&Request::Stream).unwrap();
        assert!(matches!(
            worker
                .response(CALL_TIMEOUT, &|| false, &mut |_, _| {})
                .unwrap(),
            Response::Ack
        ));
        for request in [
            Request::Audio {
                samples: vec![0.; 2000],
            },
            Request::Finish,
        ] {
            worker.send(&request).unwrap();
            let started = Instant::now();
            let response = worker
                .response(Duration::from_millis(150), &|| false, &mut |_, _| {})
                .unwrap();
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(matches!(
                response,
                Response::Ack | Response::Complete { .. }
            ));
        }
    }
    #[test]
    fn oversized_capture_callback_drains_through_bounded_ipc_frames() {
        let (_root, c) = config("test-echo");
        let mut worker = Inference::load(&c, || false).unwrap();
        let (send, receive) = crate::stream_queue::channel();
        // This would exceed the 2 MB JSON frame limit if sent as one request.
        let count = 400_001;
        send.try_send(StreamInput::audio(vec![0.123_456_7; count]))
            .unwrap();
        send.try_send(StreamInput::Finish).unwrap();
        assert_eq!(
            worker
                .stream(&receive, || false, |_, _| {})
                .unwrap()
                .as_deref(),
            Some("synthetic streaming transcript")
        );
        assert_eq!(worker.last_ack_samples, count as u64);
        assert_eq!(receive.backlog_seconds(), 0.);
        assert_eq!(worker.timing_records.last().unwrap()["feed_calls"], 26);
    }
    #[test]
    fn slow_stream_drains_large_backlog_and_warns_once() {
        let (_root, c) = config("test-slow-stream");
        let mut worker = Inference::load(&c, || false).unwrap();
        let pid = worker.child.id();
        let _context = crate::logging::context(serde_json::json!({"job_id":987}));
        let (send, receive) = crate::stream_queue::channel();
        for _ in 0..320 {
            send.try_send(StreamInput::audio(vec![0.; 200])).unwrap();
        }
        send.try_send(StreamInput::Finish).unwrap();
        assert_eq!(
            worker
                .stream(&receive, || false, |_, _| {})
                .unwrap()
                .as_deref(),
            Some("synthetic streaming transcript")
        );
        assert_eq!(worker.child.id(), pid);
        assert_eq!(worker.last_ack_samples, 64_000);
        assert_eq!(worker.inference_attempts, 1);
        let warnings: Vec<_> = worker
            .timing_records
            .iter()
            .filter(|r| r["event"] == "stream_lagging")
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["job_id"], 987);
        assert!(warnings[0]["backlog_seconds"].as_f64().unwrap() > 3.);
        assert!(warnings[0]["recent_real_time_factor"].as_f64().unwrap() > 1.);
        let summary = worker.timing_records.last().unwrap();
        assert_eq!(summary["event"], "stream_timing_summary");
        assert_eq!(summary["final_backlog_seconds"], 0.);
        assert_eq!(summary["backlog_at_finish_seconds"], 4.);
        assert!(summary["catch_up_ms"].as_f64().unwrap() >= 5000.);
        assert_eq!(summary["outcome"], "completed");
    }
    #[test]
    fn stream_summary_survives_cancellation_and_worker_failure() {
        for fail in [false, true] {
            let (_root, c) = config("test-echo");
            let mut worker = Inference::load(&c, || false).unwrap();
            let (send, receive) = crate::stream_queue::channel();
            if fail {
                worker.child.kill().unwrap();
                worker.child.wait().unwrap();
            } else {
                send.send(StreamInput::Cancel).unwrap();
            }
            let result = worker.stream(&receive, || false, |_, _| {});
            assert_eq!(result.is_err(), fail);
            let summary = worker.timing_records.last().unwrap();
            assert_eq!(summary["event"], "stream_timing_summary");
            assert_eq!(
                summary["outcome"],
                if fail { "failed" } else { "cancelled" }
            );
            assert_eq!(summary["feed_calls"], 0);
        }
    }
    #[test]
    fn cancelled_call_retains_cancelled_summary() {
        let (_root, c) = config("test-echo");
        let mut worker = Inference::load(&c, || false).unwrap();
        let (_send, receive) = crate::stream_queue::channel();
        assert!(worker.stream(&receive, || true, |_, _| {}).is_err());
        assert_eq!(
            worker.timing_records.last().unwrap()["outcome"],
            "cancelled"
        );
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
    #[test]
    #[ignore = "requires an explicitly supplied cached GGUF model and synthetic speech fixture"]
    fn native_latency_probe() {
        let model = std::env::var("VOICE_LATENCY_MODEL")
            .expect("Set VOICE_LATENCY_MODEL to a cached Parakeet GGUF file");
        assert!(std::path::Path::new(&model).is_file());
        let (_root, config) = config(&model);
        let profile = if cfg!(feature = "vulkan") {
            "vulkan"
        } else {
            "standard"
        };
        let config = config
            .changed(&serde_json::json!({"performance":{"profile":profile,"threads":0}}))
            .unwrap();
        let _context = crate::logging::context(serde_json::json!({"job_id":9001}));
        let mut worker = Inference::load(&config, || false).unwrap();
        let pid = worker.child.id();
        assert_eq!(
            worker.profile, profile,
            "Probe must exercise requested backend"
        );
        let warmup = worker
            .timing_records
            .iter()
            .find(|r| r["event"] == "native_timing" && r["stage"] == "warmup")
            .expect("warmup timing");
        assert!(warmup["encode_ms"].as_f64().unwrap() > 0.);
        let fixture = _root.path().join("speech.flac");
        std::fs::write(&fixture, include_bytes!("../assets/benchmarks/speech.flac")).unwrap();
        let mut samples = crate::engine::read_audio(&fixture, false).unwrap();
        samples.truncate(49_123);
        let mut baseline_text = None;
        for iteration in 0..2 {
            if iteration == 1 {
                worker.last_completed = Instant::now() - Duration::from_secs(61);
            }
            let (send, receive) = crate::stream_queue::channel();
            let audio = samples.clone();
            let producer = std::thread::spawn(move || {
                for chunk in audio.chunks(1024) {
                    send.send(StreamInput::audio(chunk.to_vec())).unwrap();
                    std::thread::sleep(Duration::from_millis(64));
                }
                send.send(StreamInput::Finish).unwrap();
            });
            let offset = worker.timing_records.len();
            let text = worker.stream(&receive, || false, |_, _| {}).unwrap();
            producer.join().unwrap();
            assert!(text.is_some());
            if iteration == 0 {
                baseline_text = text.clone();
            } else {
                assert_eq!(
                    text, baseline_text,
                    "idle synthetic stream must not change real text"
                );
            }
            let records = &worker.timing_records[offset..];
            let begin = records
                .iter()
                .find(|r| r["event"] == "native_timing" && r["stage"] == "stream_begin")
                .unwrap();
            assert_eq!(
                records
                    .iter()
                    .filter(|r| r["event"] == "native_timing" && r["stage"] == "idle_warmup")
                    .count(),
                iteration
            );
            assert!(records.iter().any(|r| r["event"] == "paging_snapshot"
                && r["stage"] == "stream_finalize"
                && r["phase"] == "after"));
            assert_eq!(begin["receiving_operation"], "stream_start");
            for counter in [
                "baseline_mel_ms",
                "baseline_encode_ms",
                "baseline_decode_ms",
            ] {
                assert_eq!(
                    begin[counter], 0.,
                    "stream counters must exclude warmup and prior streams"
                );
            }
            assert!(records.iter().any(|r| r["event"] == "native_timing"
                && r["stage"] == "stream_feed"
                && r["encode_ms"].as_f64().unwrap() > 0.));
            assert!(records.iter().any(|r| r["stage"] == "stream_finalize"));
            assert!(records
                .iter()
                .all(|r| r["job_id"] == 9001 && r["worker_pid"] == pid));
            assert_eq!(worker.child.id(), pid);
        }
        assert_eq!(
            worker.inference_attempts, 2,
            "warmup is not a user inference attempt"
        );
        println!(
            "{}",
            serde_json::json!({"profile":profile,"records":worker.timing_records})
        );
    }
}
