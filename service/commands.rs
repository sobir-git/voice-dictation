use super::*;
impl Daemon {
    pub(super) fn config_reply(&mut self, id: u64) {
        self.reply(
            id,
            json!({
            "type":"config","config":self.config.data,"microphones":audio::microphones()}
            ),
        );
    }
    pub(super) fn command(&mut self, id: u64, message: &Value) -> Result<()> {
        let request = crate::protocol::Request::parse(message)?;
        let _context = crate::logging::context(
            json!({"client_id":id,"client_session":request.client_session,"request_id":request.request_id,"command":request.cmd}),
        );
        log::info!("Command received");
        let previous = self.request_id;
        self.request_id = request.request_id;
        let result = self.dispatch_command(id, message, request.cmd);
        self.request_id = previous;
        log::info!("Command dispatched: success={}", result.is_ok());
        result
    }
    fn dispatch_command(
        &mut self,
        id: u64,
        message: &Value,
        kind: crate::protocol::CommandName,
    ) -> Result<()> {
        use crate::protocol::CommandName::*;
        let command = message["cmd"].as_str().unwrap_or("");
        log::debug!("Command: client={id} command={command}");
        if self.configuring
            && !matches!(
                command,
                "get_state" | "get_config" | "history" | "diagnostics" | "quit" | "cancel"
            )
        {
            bail!("Settings are being saved. Try again shortly.");
        }
        if self.benchmarking
            && matches!(
                command,
                "save_config"
                    | "reload_config"
                    | "retry_history"
                    | "start_recording"
                    | "test_microphone"
            )
        {
            bail!("Finish or cancel the benchmark first");
        }
        if self.io_pending >= 64
            && !matches!(
                kind,
                GetState | Cancel | StopRecording | AbortRecording | Quit
            )
        {
            bail!("Storage is busy. Try again shortly.");
        }
        match kind {
            BenchmarkStatus => {
                let mut status = self.benchmark_status.clone();
                status["reports"] = optimization::reports(&self.config);
                self.reply(id, status);
            }
            CancelBenchmark => {
                if self.benchmark_client != Some(id) {
                    bail!("This client does not own the benchmark");
                }
                self.benchmark_cancel.store(true, Ordering::Relaxed);
            }
            Benchmark => {
                if self.benchmarking
                    || self.jobs.busy()
                    || self.jobs.outputting()
                    || self.model_state != ModelState::Ready
                    || self.test.is_some()
                {
                    bail!("Wait for the model and pending dictation before benchmarking");
                }
                let candidate = self.config.changed(&json!({
                    "transcription":message["transcription"], "performance":message["performance"]
                }
))?;
                optimization::check_profile(&candidate)?;
                let durations = optimization::durations(&message["durations"])?;
                self.benchmark_cancel.store(false, Ordering::Relaxed);
                self.worker
                    .try_send(Work::Benchmark(
                        candidate,
                        durations,
                        self.benchmark_cancel.clone(),
                    ))
                    .map_err(|_| anyhow::anyhow!("Speech worker is busy"))?;
                self.benchmarking = true;
                self.benchmark_client = Some(id);
                self.model_state = ModelState::Unavailable;
                self.benchmark_status = json!({
                "type":"benchmark","running":true,"message":"Starting local benchmark. Dictation resumes when it finishes."}
                );
                self.broadcast(self.benchmark_status.clone());
                self.broadcast_state();
            }
            StartRecording => {
                if id == 0 || !self.clients.contains_key(&id) {
                    bail!("Recording requires a connected client")
                }
                if !self.jobs.can_record() || self.key_capture.is_some() || self.test.is_some() {
                    bail!("Dictation is paused or busy")
                }
                let node = message["pipewire_node"].as_str().unwrap_or("");
                if node.is_empty() || node.len() > 256 || node.chars().any(char::is_control) {
                    bail!("A microphone node is required")
                }
                let config = self.config.changed(&json!({
                "audio":{
                                    "device":"pipewire", "pipewire_node":node
                                }
                }
                ))?;
                self.start_recording_from(config, Some(id));
                if self.jobs.recording().is_none() {
                    bail!("Could not start recording: {}", self.last_error)
                }
                self.reply(
                    id,
                    json!({
                    "type":"recording_started"}
                    ),
                );
            }
            StopRecording | AbortRecording => {
                if self
                    .jobs
                    .recording()
                    .is_none_or(|job| self.jobs.active[&job].owner != Some(id))
                {
                    bail!("This client does not own the recording")
                }
                if message["cmd"] == "abort_recording" {
                    self.cancel_remote_recording();
                } else {
                    self.stop_recording();
                }
                self.reply(
                    id,
                    json!({
                    "type":"recording_stopped"}
                    ),
                );
            }
            GetState => self.reply(id, self.state()),
            GetConfig => {
                self.config_reply(id);
                self.effect(move |history| {
                    Event::Reply(id,history.recent(1,"").map(|items|json!({
"type":"transcription","text":items.first().map(|i|i["text"].clone()).unwrap_or(json!(""))}
)))
                })?;
            }
            History => {
                let query = message["search"].as_str().unwrap_or("").to_owned();
                if query.len() > 4096 {
                    bail!("Search is too long");
                }
                self.effect(move |history| {
                    Event::Reply(
                        id,
                        history.recent(100, &query).map(|items| {
                            json!({
                            "type":"history","search":query,"items":items}
                            )
                        }),
                    )
                })?;
            }
            FavoriteHistory => {
                let row = message["id"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Expected history ID"))?;
                let favorite = message["favorite"]
                    .as_bool()
                    .ok_or_else(|| anyhow::anyhow!("Expected favorite boolean"))?;
                self.effect(move |history| {
                    Event::Reply(
                        id,
                        history.set_favorite(row, favorite).map(|_| {
                            json!({
                            "type":"history_changed"}
                            )
                        }),
                    )
                })?;
            }
            DeleteHistory => {
                let row = message["id"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Expected history ID"))?;
                if self.jobs.active.values().any(|j| j.history_id == Some(row)) {
                    bail!("Wait for this dictation to finish before deleting it");
                }
                self.effect(move |history| {
                    Event::Reply(
                        id,
                        (|| -> Result<Value> {
                            if let Some(path) = history.remove(row)? {
                                match std::fs::remove_file(path) {
                                    Ok(()) => {}
                                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                    Err(e) => return Err(e.into()),
                                }
                            }
                            Ok(json!({
                            "type":"history_changed"}
                            ))
                        })(),
                    )
                })?;
            }
            RetryHistory => {
                let row = message["id"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Expected history ID"))?;
                if self.jobs.active.values().any(|j| j.history_id == Some(row)) {
                    self.reply(
                        id,
                        json!({
                        "type":"retry_history","status":"busy","id":row}
                        ),
                    );
                    return Ok(());
                }
                if self.jobs.active.len() >= 4 {
                    bail!("Transcription queue is full");
                }
                let config = retry_config(&self.config, message)?;
                let job = self.jobs.insert(config, Stage::Resolving, None, Some(row));
                if let Err(error) = self.effect(move |history| {
                    Event::RetryReady(
                        job,
                        (|| -> Result<std::path::PathBuf> {
                            let item = history
                                .get(row)?
                                .ok_or_else(|| anyhow::anyhow!("Recording no longer exists"))?;
                            let path = item["audio_path"]
                                .as_str()
                                .filter(|p| !p.is_empty())
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "This older history item has no saved recording"
                                    )
                                })?;
                            let path = std::path::PathBuf::from(path);
                            if !path.is_file() {
                                bail!("The saved recording is missing");
                            }
                            Ok(path)
                        })(),
                    )
                }) {
                    self.terminal(job, "queue_rejected");
                    return Err(error);
                }
                self.last_error.clear();
                self.reply(
                    id,
                    json!({
                    "type":"retry_history","status":"accepted","id":row,"job_id":job}
                    ),
                );
                self.broadcast_state();
            }
            PlayHistory => {
                let row = message["id"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("Expected history ID"))?;
                self.effect(move |history| {
                    Event::Reply(
                        id,
                        (|| -> Result<Value> {
                            if let Some(item) = history.get(row)? {
                                if let Some(path) = item["audio_path"].as_str() {
                                    open_path(std::path::Path::new(path))?;
                                }
                            }
                            Ok(json!({
                            "type":"history_playing","id":row}
                            ))
                        })(),
                    )
                })?;
            }
            OpenRecordings => {
                let path = self.config.data_dir.join("recordings");
                self.effect(move |_| {
                    Event::Reply(
                        id,
                        (|| -> Result<Value> {
                            std::fs::create_dir_all(&path)?;
                            open_path(&path)?;
                            Ok(json!({
                            "type":"recordings_opened"}
                            ))
                        })(),
                    )
                })?;
            }
            ToggleListening | SetListening => {
                if self.key_capture.is_some() {
                    bail!("Finish hotkey capture first")
                }
                self.jobs.listening = if message["cmd"] == "toggle_listening" {
                    !self.jobs.listening
                } else {
                    message["value"]
                        .as_bool()
                        .ok_or_else(|| anyhow::anyhow!("Expected a boolean"))?
                };
                if !self.jobs.listening {
                    self.stop_recording();
                }
                self.broadcast_state();
            }
            Cancel => self.cancel(),
            ClearError => {
                self.last_error.clear();
                self.broadcast_state();
            }
            SaveConfig | ReloadConfig | SetLogLevel => {
                if self.jobs.busy() || self.key_capture.is_some() || self.test.is_some() {
                    bail!("Wait for dictation or microphone/hotkey testing to finish, then save settings again.")
                }
                let previous = self.config.clone();
                let message = message.clone();
                self.effect(move |_| {
                    Event::ConfigSaved(
                        id,
                        (|| -> Result<(Config, String)> {
                            let candidate = prepare_config(&previous, &message)?;
                            let resolved = output::resolve(candidate.string("output", "method"));
                            candidate.save()?;
                            Ok((candidate, resolved))
                        })(),
                    )
                })?;
                self.configuring = true;
                self.broadcast_state();
            }
            CaptureHotkey => {
                if self.jobs.busy() || self.key_capture.is_some() || self.test.is_some() {
                    bail!("Finish dictation or testing before changing the hotkey")
                }
                let keys = self
                    .hotkeys
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Hotkeys are disabled in probe mode"))?;
                self.key_capture = Some(KeyCapture {
                    client: id,
                    restore: self.jobs.listening,
                    deadline: Instant::now() + Duration::from_secs(10),
                });
                self.jobs.listening = false;
                keys.capture.store(true, Ordering::Relaxed);
                self.reply(
                    id,
                    json!({
                    "type":"hotkey_waiting"}
                    ),
                );
                self.broadcast_state();
            }
            TestMicrophone => {
                if self.jobs.busy() || self.key_capture.is_some() {
                    bail!("Finish dictation before testing the microphone")
                }
                if self.test.is_some() {
                    self.stop_test();
                    self.reply(id,json!({
"type":"microphone_test","level":0,"message":"Microphone test stopped.","done":true}
));
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
            ExportDiagnostics => {
                anyhow::ensure!(!self.exporting, "A diagnostic export is already running");
                let config = self.config.clone();
                let send = self.server.send.clone();
                let request_id = self.request_id;
                std::thread::Builder::new().name("diagnostic-export".into()).spawn(move || {
                    let result=(||->Result<Value>{
                        crate::logging::flush();
                        let executable=std::env::current_exe()?;
                        let output=crate::process::run(std::process::Command::new("python3")
                            .arg(crate::companion::project().join("tools/diagnostics.py"))
                            .arg("--log-path").arg(crate::config::expand(config.string("logging","file")))
                            .arg("--binary-dir").arg(executable.parent().unwrap()),None,Duration::from_secs(45))?;
                        let report:Value=serde_json::from_slice(&output)?;
                        Ok(json!({"type":"diagnostics_exported","path":report["path"],"bytes":report["bytes"]}))
                    })();
                    let _=send.send(Event::Exported(id,request_id,result));
                })?;
                self.exporting = true;
                self.io_pending += 1;
            }
            Diagnostics => {
                let state = self.state();
                let config = self.config.clone();
                self.effect(move|_|Event::Reply(id,(||->Result<Value>{
                    let tools=["arecord","ffmpeg","xdotool","ydotool","dotool","wtype"].map(|tool|json!({
"name":tool,"installed":crate::process::exists(tool)}
));
                    let text=json!({
"logging_health":crate::logging::health(),"full_export":"Use Export report to collect rotated component logs, journal and crash metadata.","session":std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),"socket":crate::ipc::socket_path(),"daemon":state,"audio":config.data["audio"],"transcription":config.data["transcription"],"microphones":audio::microphones(),"engine":"Supervised speech worker (transcribe.cpp / CTranslate2)","tools":tools}
);
                    Ok(json!({
"type":"diagnostics","text":serde_json::to_string_pretty(&text)?,"logs":crate::logging::recent(&config)}
))
                }
)()))?;
            }
            Quit => {
                // Existing adapters must not automatically restart an intentional quit.
                std::fs::write(crate::ipc::stopped_path(), b"")?;
                self.quitting = true;
            }
        }
        Ok(())
    }
}
