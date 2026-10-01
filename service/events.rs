use super::*;

impl Daemon {
    pub(super) fn event(&mut self, event: Event) {
        match event {
            Event::Storage(request_id, event) => {
                self.io_pending = self.io_pending.checked_sub(1).expect("unmatched storage result");
                let previous = self.request_id;
                self.request_id = request_id;
                let _context=crate::logging::context(json!({"request_id":request_id,"component":"storage_completion"}));
                self.event(*event);
                self.request_id = previous;
            }
            Event::Exported(id, request_id, result) => {
                self.exporting=false;
                self.event(Event::Storage(request_id,Box::new(Event::Reply(id,result))));
            }
            Event::Reply(id, result) => match result {
                Ok(value) => self.reply(id, value),
                Err(error) => self.reply(id, json!({"type":"error", "message":format!("{error:#}")})),
            },
            Event::Connect(id, client) => {
                log::info!("Client connected: client={id}");
                if self.clients.len() < 8 {
                    self.clients.insert(id, client);
                    self.reply(id, self.state());
                }
            }
            Event::Disconnect(id) => {
                let before=self.state();
                self.disconnect(id);
                self.flush_output();
                if self.state()!=before {self.broadcast_state();}
            }
            Event::Command(id, message) => {
                if let Err(error) = self.command(id, &message) {
                    log::warn!("Command failed: client={id} request_id={} command={} error={error:#}", message["request_id"], message["cmd"]);
                    let response = json!({"type":"error", "message":format!("{error:#}"), "request_id":message["request_id"]});
                    self.reply(id, response);
                    if id == 0 { self.error(format!("{error:#}")); }
                }
            }
            Event::Key(KeyEvent::Down) => self.start_recording(),
            Event::Key(KeyEvent::Up) => {
                if self.jobs.recording().is_some_and(|id| self.jobs.active[&id].owner.is_none()) {
                    self.stop_recording();
                }
            }
            Event::Key(KeyEvent::Missing) => self.error("No readable keyboard supports the hotkey. Check the input group; waiting for a keyboard.".into()),
            Event::Key(KeyEvent::Captured(key)) => {
                if let Some(capture) = &self.key_capture {
                    self.reply(capture.client, json!({"type":"hotkey", "key":key}));
                }
                self.end_key_capture();
            }
            Event::Level(id, level, db) => self.audio_level(id, level, db),
            Event::Preview(id, committed, tentative) => {
                if self.jobs.recording() == Some(id) {
                    let value = json!({"type":"transcription_preview", "committed":committed, "tentative":tentative, "job_id":id});
                    for client in self.clients.values() { let _ = client.try_send(value.clone()); }
                }
            }
            Event::CaptureStarted(id, result) => self.capture_started(id, result),
            Event::CaptureFault(id, error) => {
                if self.jobs.recording() == Some(id) {
                    self.jobs.active.get_mut(&id).unwrap().recover_stream(&error);
                    self.error(error);
                    self.stop_recording();
                }
            }
            Event::Finalized(id, result) => self.capture_finalized(id, result),
            Event::Saved(id, result) => self.capture_saved(id, result),
            Event::WorkerTranscribed(id, attempt, result) => {
                if self.jobs.active.get(&id).is_some_and(|job| Arc::ptr_eq(&job.cancelled, &attempt)) {
                    self.transcribed(id, result);
                } else {
                    log::info!("Discarded superseded inference result: job={id}");
                }
            }
            #[cfg(test)]
            Event::Transcribed(id, result) => self.transcribed(id, result),
            Event::Persisted(id, result) => self.persisted(id, result),
            Event::Delivered(id, result) => {
                if self.jobs.active.get(&id).is_some_and(|job| job.stage == Stage::Delivering) {
                    match result {
                        Ok(()) => {let outcome=if self.jobs.active[&id].cancelled.load(Ordering::Acquire){"cancelled"}else{"completed"};self.terminal(id,outcome);},
                        Err(error) => self.fail(id, format!("Text output failed: {error:#}")),
                    }
                    self.flush_output();
                    self.broadcast_state();
                }
            }
            Event::RetryReady(id, result) => self.retry_ready(id, result),
            Event::ConfigSaved(id, result) => self.configuration_saved(id, result),
            Event::ModelLoading(identity)=>{
                if identity==optimization::identity(&self.config){self.model_state=ModelState::Loading;self.broadcast_state();}
            }
            Event::WorkerUnavailable(identity)=>{
                if identity==optimization::identity(&self.config){self.model_state=ModelState::Unavailable;self.runtime_profile.clear();self.broadcast_state();}
            }
            Event::Model(identity, result) => {
                if identity == optimization::identity(&self.config) {
                    self.model_state=if result.is_ok(){ModelState::Ready}else{ModelState::Unavailable};
                    match result {
                        Ok(profile) => self.runtime_profile = profile,
                        Err(error) => self.error(format!("Model load failed: {error:#}")),
                    }
                    self.broadcast_state();
                }
            }
            Event::Benchmark(value) => {
                self.benchmark_status = value.clone();
                self.broadcast(value);
            }
            Event::BenchmarkDone(result) => self.benchmark_finished(result),
            Event::Test(token, message) => {
                if let Some(test) = self.test.as_ref().filter(|test| test.token == token) {
                    let id = test.client;
                    let done = message["done"] == true;
                    self.reply(id, message);
                    if done { self.stop_test(); }
                }
            }
            Event::Stop => {}
        }
    }

    fn audio_level(&mut self, id: JobId, level: f32, db: f32) {
        let Some(job) = self
            .jobs
            .active
            .get_mut(&id)
            .filter(|job| job.stage.recording())
        else {
            return;
        };
        job.capture_ready = true;
        let value = json!({"type":"audio_level", "level":level, "db":db, "capture_ready":true});
        // Meters are disposable; terminal state uses reliable delivery or reconnect.
        for client in self.clients.values() {
            let _ = client.try_send(value.clone());
        }
        let mut state = self.state();
        state["level"] = json!(level);
        if let Some(companion) = &self.companion {
            companion.update(state, &self.config);
        }
    }

    fn capture_started(&mut self, id: JobId, result: Result<Capture>) {
        match result {
            Ok(capture) => {
                let job = self.jobs.active.get_mut(&id).filter(|job| {
                    job.capture.is_none()
                        && matches!(job.stage, Stage::Starting | Stage::Finalizing)
                });
                if let Some(job) = job {
                    job.capture = Some(capture);
                    if job.stage == Stage::Starting {
                        job.transition(Stage::Recording);
                    } else {
                        self.finalize_capture(id);
                    }
                } else {
                    self.finish_capture(id, capture);
                }
            }
            Err(error) => self.fail(id, format!("Recording failed: {error:#}")),
        }
        self.broadcast_state();
    }

    fn capture_finalized(
        &mut self,
        id: JobId,
        result: Result<(std::path::PathBuf, Option<String>)>,
    ) {
        if let Some(job) = self
            .jobs
            .active
            .get_mut(&id)
            .filter(|job| job.stage == Stage::Finalizing)
        {
            job.transition(Stage::Saving);
        }
        // Even late completions after cancellation must be indexed in history.
        // Cancellation governs inference/output, never ownership of captured audio.
        match result {
            Ok((file, warning)) => {
                let directory = self.config.data_dir.join("recordings");
                let effect = move |history: &History| {
                    Event::Saved(
                        id,
                        crate::storage::save_recording(history, &directory, file, warning),
                    )
                };
                if let Err(error) = self.effect(effect) {
                    self.error(format!(
                        "Could not index recording; audio retained on disk: {error:#}"
                    ));
                    self.fail(id, format!("Could not save recording: {error:#}"));
                }
            }
            Err(error) => {
                self.error(format!(
                    "Recording finalization failed; capture file retained in recordings: {error:#}"
                ));
                self.fail(id, format!("Recording failed: {error:#}"));
            }
        }
    }

    fn capture_saved(&mut self, id: JobId, result: Result<SavedRecording>) {
        let Some(job) = self
            .jobs
            .active
            .get_mut(&id)
            .filter(|job| job.stage == Stage::Saving)
        else {
            match result {
                Ok(saved) => {
                    log::info!(
                        "Interrupted recording retained: history={}",
                        saved.history_id
                    );
                    self.broadcast(json!({"type":"history_changed"}));
                }
                Err(error) => self.error(format!(
                    "Could not index interrupted recording; audio retained: {error:#}"
                )),
            }
            return;
        };
        let saved = match result {
            Ok(saved) => saved,
            Err(error) => {
                self.fail(id, format!("Could not save recording: {error:#}"));
                return;
            }
        };
        job.history_id = Some(saved.history_id);
        job.transition(Stage::Transcribing);
        let has_result = job.result.is_some();
        if let Some(stream) = job.stream.take() {
            if let Err(error) = stream.try_send(StreamInput::Finish) {
                if !has_result {
                    let reason = match error {
                        mpsc::TrySendError::Full(_) => "finish_audio_duration_cap",
                        mpsc::TrySendError::Disconnected(_) => "finish_worker_disconnected",
                    };
                    job.recover_stream(reason);
                } else {
                    log::info!("Stream finish unnecessary: job={id} result_already_received=true");
                }
            } else {
                // Keep a marker so this job waits for its stream result.
                job.stream = Some(stream);
            }
        }
        let work = if !has_result && job.stream.is_none() {
            Some(Work::Transcribe {
                id,
                generation: job.generation,
                cancelled: job.cancelled.clone(),
                path: saved.path,
                config: job.config.clone(),
                requested: job.finished.get().copied().unwrap_or(job.started),
                queued: Instant::now(),
            })
        } else {
            None
        };
        if let Some(warning) = saved.warning {
            self.error(format!("{warning} Recovering the audio captured so far."));
        }
        if has_result {
            self.persist(id);
        } else if let Some(work) = work {
            if self.worker.try_send(work).is_err() {
                self.fail(id, "Transcription queue is full".into());
            }
        }
        self.broadcast_state();
    }

    fn transcribed(&mut self, id: JobId, result: Transcript) {
        let Some(job) = self.jobs.active.get_mut(&id) else {
            return;
        };
        if job.result.is_some() {
            return;
        }
        log::info!(
            "Inference result: job={id} success={} model={} elapsed={:.3}s",
            result.text.is_ok(),
            result.model,
            result.seconds
        );
        job.result = Some(result);
        match job.stage {
            Stage::Starting | Stage::Recording => self.stop_recording(),
            Stage::Transcribing => self.persist(id),
            _ => {} // Capture must be saved before its inference result is committed.
        }
    }

    fn persisted(&mut self, id: JobId, result: Result<()>) {
        let Some(job) = self
            .jobs
            .active
            .get_mut(&id)
            .filter(|job| job.stage == Stage::Persisting)
        else {
            return;
        };
        let transcript = job.result.as_ref().unwrap();
        let message = match &transcript.text {
            Ok(text) if !text.is_empty() => {
                let value = json!({"type":"transcription", "text":text, "duration":transcript.seconds, "model":transcript.model, "job_id":id, "history_id":job.history_id});
                job.transition(Stage::Ready);
                self.broadcast(value);
                None
            }
            Ok(_) => Some("No speech detected. Check the microphone in Settings.".into()),
            Err(error) => Some(format!("Transcription failed: {error}")),
        };
        if let Err(error) = result {
            self.error(format!("Could not save transcription history: {error:#}"));
        }
        if let Some(message) = message {
            self.fail(id, message);
        } else {
            self.flush_output();
            self.broadcast_state();
        }
    }

    fn retry_ready(&mut self, id: JobId, result: Result<std::path::PathBuf>) {
        let Some(job) = self
            .jobs
            .active
            .get_mut(&id)
            .filter(|job| job.stage == Stage::Resolving)
        else {
            return;
        };
        match result {
            Ok(path) => {
                job.transition(Stage::Transcribing);
                let work = Work::Transcribe {
                    id,
                    generation: job.generation,
                    cancelled: job.cancelled.clone(),
                    path,
                    config: job.config.clone(),
                    requested: job.started,
                    queued: Instant::now(),
                };
                if self.worker.try_send(work).is_err() {
                    self.fail(id, "Transcription queue is full".into());
                }
            }
            Err(error) => self.fail(id, format!("Retry failed: {error:#}")),
        }
    }

    fn configuration_saved(&mut self, id: u64, result: Result<(Config, String)>) {
        self.configuring = false;
        match result {
            Ok((config, resolved)) => {
                let changed =
                    optimization::identity(&config) != optimization::identity(&self.config);
                self.config = config;
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
                if changed {
                    self.model_state = ModelState::Loading;
                    if self
                        .worker
                        .try_send(Work::Load(self.config.clone()))
                        .is_err()
                    {
                        self.error("Model worker is busy; retry loading the model.".into());
                    }
                }
                self.broadcast(json!({"type":"config_reloaded"}));
                self.config_reply(id);
            }
            Err(error) => self.reply(
                id,
                json!({"type":"error", "message":format!("Could not save settings: {error:#}")}),
            ),
        }
        self.broadcast_state();
    }

    fn benchmark_finished(&mut self, result: Result<Value>) {
        self.benchmarking = false;
        self.benchmark_client = None;
        self.benchmark_status = match result {
            Ok(report) => {
                json!({"type":"benchmark", "running":false, "message":"Finished. Results saved locally; settings were not changed.", "report":report})
            }
            Err(error) => {
                json!({"type":"benchmark", "running":false, "message":format!("{error:#}")})
            }
        };
        self.broadcast(self.benchmark_status.clone());
        if self
            .worker
            .try_send(Work::Load(self.config.clone()))
            .is_err()
        {
            self.error("Could not restore the selected model".into());
        }
        self.broadcast_state();
    }
}
