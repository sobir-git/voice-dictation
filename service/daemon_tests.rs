use super::*;
fn isolated() -> (tempfile::TempDir, Daemon) {
    let root = tempfile::tempdir().unwrap();
    let config = Config::at(root.path())
        .unwrap()
        .changed(&json!({
                    "output":{
        "method":"none"}
        ,
                    "notifications":{
        "enabled":false,"audio_feedback":false}
                }
        ))
        .unwrap();
    let server = Server::bind(root.path().join("run/daemon.sock")).unwrap();
    let daemon = Daemon::with_server(config, true, false, server).unwrap();
    (root, daemon)
}
impl Daemon {
    fn pump(&mut self, mut done: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !done(self) {
            assert!(Instant::now() < deadline, "coordinator did not settle");
            match self.server.receive.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => self.event(event),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => panic!("completion channel closed: {error}"),
            }
        }
    }
    fn test_command(&mut self, id: u64, message: &Value) -> Result<()> {
        self.command(id, message)?;
        self.pump(|d| d.io_pending == 0 && !d.configuring);
        Ok(())
    }
    fn seed(&mut self, stage: Stage, history: Option<i64>) -> JobId {
        self.jobs.insert(self.config.clone(), stage, None, history)
    }
    fn result(&mut self, id: JobId, text: Result<&str, &str>) {
        self.event(Event::Transcribed(
            id,
            Transcript {
                text: text.map(String::from).map_err(String::from),
                model: "synthetic-model".into(),
                seconds: 0.25,
            },
        ));
    }
}
#[test]
fn benchmark_blocks_competing_work_and_cancels_without_changing_settings() {
    let (_root, mut daemon) = isolated();
    let original = daemon.config.data.clone();
    daemon.benchmarking = true;
    daemon.benchmark_client = Some(1);
    assert!(daemon
        .test_command(
            1,
            &json!({
            "cmd":"save_config","config":{
            }
            }
            )
        )
        .is_err());
    assert!(daemon
        .test_command(
            1,
            &json!({
            "cmd":"retry_history","id":1}
            )
        )
        .is_err());
    assert!(daemon
        .test_command(
            1,
            &json!({
            "cmd":"benchmark","durations":[5]}
            )
        )
        .is_err());
    daemon.start_recording();
    assert!(daemon.jobs.recording().is_none());
    assert!(daemon
        .test_command(
            2,
            &json!({
            "cmd":"cancel_benchmark"}
            )
        )
        .is_err());
    assert!(!daemon.benchmark_cancel.load(Ordering::Relaxed));
    daemon
        .test_command(
            1,
            &json!({
            "cmd":"cancel_benchmark"}
            ),
        )
        .unwrap();
    assert!(daemon.benchmark_cancel.load(Ordering::Relaxed));
    assert_eq!(daemon.config.data, original);
}
#[test]
fn retry_carries_unsaved_performance_settings() {
    let root = tempfile::tempdir().unwrap();
    let saved = Config::at(root.path()).unwrap();
    let selected = retry_config(
        &saved,
        &json!({
        "performance":{
        "profile":"standard","threads":2}
        }
        ),
    )
    .unwrap();
    assert_eq!(selected.number("performance", "threads"), 2);
    assert_eq!(saved.number("performance", "threads"), 0);
    assert_ne!(
        optimization::identity(&saved),
        optimization::identity(&selected)
    );
}
#[test]
fn switching_models_restores_each_saved_performance_setting() {
    let (_root, mut daemon) = isolated();
    let (worker, receive) = mpsc::sync_channel(5);
    daemon.worker = worker;
    daemon
        .test_command(
            1,
            &json!({
            "cmd":"save_config","remember_performance":true,
                                "config":{
            "performance":{
            "profile":"standard","threads":4}
            }
            }
            ),
        )
        .unwrap();
    assert!(matches!(receive.try_recv().unwrap(), Work::Load(_)));
    daemon
        .test_command(
            1,
            &json!({
            "cmd":"save_config","remember_performance":true,"config":{
                                "transcription":{
            "model":"base.en"}
            ,
                                "performance":{
            "profile":"adaptive","threads":8}
            }
            }
            ),
        )
        .unwrap();
    assert!(matches!(receive.try_recv().unwrap(), Work::Load(_)));
    daemon
        .test_command(
            1,
            &json!({
            "cmd":"save_config","config":{
                                "transcription":{
            "model":crate::engine::PARAKEET_MODEL}
            }
            }
            ),
        )
        .unwrap();
    let Work::Load(selected) = receive.try_recv().unwrap() else {
        panic!("Expected the restored model to load");
    };
    assert_eq!(
        optimization::current_setting(&selected),
        json!({
        "profile":"standard","threads":4}
        )
    );
    assert_eq!(
        selected.data["performance"]["by_model"]["base.en"],
        json!({
        "profile":"adaptive","threads":8}
        )
    );
}
#[test]
fn ui_model_switch_preserves_an_unsaved_outgoing_performance_draft() {
    let (_root, mut daemon) = isolated();
    let (worker, receive) = mpsc::sync_channel(5);
    daemon.worker = worker;
    daemon
        .test_command(
            1,
            &json!({
            "cmd":"save_config","remember_performance":true,"config":{
                            "transcription":{
            "model":"canary-180m-flash"}
            ,
                            "performance":{
            "profile":"standard","threads":8,"by_model":{
                                PARAKEET_MODEL:{
            "profile":"standard","threads":4}
                            }
            }
                        }
            }
            ),
        )
        .unwrap();
    let Work::Load(selected) = receive.try_recv().unwrap() else {
        panic!("Expected the selected model to load");
    };
    assert_eq!(
        selected.data["performance"]["by_model"][PARAKEET_MODEL],
        json!({
        "profile":"standard","threads":4}
        )
    );
    assert_eq!(
        optimization::current_setting(&selected),
        json!({
        "profile":"standard","threads":8}
        )
    );
}
#[test]
fn history_retry_uses_selected_settings_without_changing_saved_config() {
    let root = tempfile::tempdir().unwrap();
    let saved = Config::at(root.path()).unwrap();
    let selected = retry_config(
        &saved,
        &json!({
        "transcription":{
        "model":"canary-180m-flash"}
        }
        ),
    )
    .unwrap();
    assert_eq!(
        selected.string("transcription", "model"),
        "canary-180m-flash"
    );
    assert_eq!(saved.string("transcription", "model"), PARAKEET_MODEL);
    assert_eq!(retry_config(&saved, &json!({})).unwrap().data, saved.data);
    assert!(retry_config(
        &saved,
        &json!({
        "transcription":{
        "beam_size":0}
        }
        )
    )
    .is_err());
}
#[test]
fn history_retry_queues_selected_model_and_clears_previous_failure() {
    let (root, mut daemon) = isolated();
    let path = root.path().join("synthetic.wav");
    std::fs::write(&path, []).unwrap();
    let id = daemon.history.add_recording("", &path, 160., true).unwrap();
    let (worker, receive) = mpsc::sync_channel(5);
    daemon.worker = worker;
    daemon.last_error = "Old model failed".into();
    daemon
        .test_command(
            1,
            &json!({
            "cmd":"retry_history", "id":id,
                        "transcription":{
            "model":"canary-180m-flash"}
            }
            ),
        )
        .unwrap();
    let Work::Transcribe {
        path: queued_path,
        config,
        id: job_id,
        ..
    } = receive.try_recv().unwrap()
    else {
        panic!("Expected a history transcription");
    };
    assert_eq!(queued_path, path);
    assert_eq!(daemon.jobs.active[&job_id].history_id, Some(id));
    assert_eq!(config.string("transcription", "model"), "canary-180m-flash");
    assert_eq!(
        daemon.config.string("transcription", "model"),
        PARAKEET_MODEL
    );
    assert!(daemon.last_error.is_empty());
    assert_eq!(daemon.jobs.pending(), 1);
}
#[test]
fn canceled_inference_never_reaches_history_or_output() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Transcribing, None);
    d.cancel();
    d.result(id, Ok("cancelled words"));
    assert!(d.history.recent(100, "").unwrap().is_empty());
    assert!(!d.jobs.busy());
}
#[test]
fn completion_waits_for_earlier_job_and_recording_release() {
    let (_root, mut d) = isolated();
    let first = d.seed(Stage::Transcribing, None);
    let second = d.seed(Stage::Transcribing, None);
    let capture = d.seed(Stage::Recording, None);
    d.result(second, Ok("second"));
    d.pump(|d| d.io_pending == 0);
    assert!(!d.jobs.outputting());
    d.result(first, Ok("first"));
    d.pump(|d| d.io_pending == 0);
    assert!(!d.jobs.outputting());
    d.terminal(capture, "synthetic_stop");
    d.flush_output();
    assert_eq!(d.jobs.active[&first].stage, Stage::Delivering);
    assert_eq!(d.jobs.active[&second].stage, Stage::Ready);
    d.pump(|d| !d.jobs.busy());
    assert_eq!(d.history.recent(10, "").unwrap().len(), 2);
}
#[test]
fn output_failure_keeps_saved_text_and_releases_job() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Transcribing, None);
    let recording = d.seed(Stage::Recording, None);
    d.result(id, Ok("retained text"));
    d.pump(|d| d.io_pending == 0);
    d.terminal(recording, "synthetic_stop");
    d.jobs
        .active
        .get_mut(&id)
        .unwrap()
        .transition(Stage::Delivering);
    d.event(Event::Delivered(
        id,
        Err(anyhow::anyhow!("synthetic output failure")),
    ));
    assert_eq!(d.history.recent(1, "").unwrap()[0]["text"], "retained text");
    assert!(d.jobs.can_record());
    assert!(d.last_error.contains("output failure"));
}
#[test]
fn shutdown_drains_admitted_inference_and_storage() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Transcribing, None);
    d.server
        .send
        .send(Event::Transcribed(
            id,
            Transcript {
                text: Ok("saved on shutdown".into()),
                model: "test".into(),
                seconds: 0.1,
            },
        ))
        .unwrap();
    d.finish_shutdown();
    assert!(!d.jobs.busy());
    assert_eq!(
        d.history.recent(1, "").unwrap()[0]["text"],
        "saved on shutdown"
    );
}
#[test]
fn busy_settings_never_write_and_reload_keeps_pause() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Transcribing, None);
    let request = json!({
    "cmd":"save_config","config":{
    "ui":{
    "cursor_indicator":true}
    }
    }
    );
    assert!(d.command(1, &request).is_err());
    assert!(!d.config.path.exists());
    d.terminal(id, "cancelled");
    d.jobs.listening = false;
    d.test_command(1, &request).unwrap();
    assert!(!d.jobs.listening);
    assert!(d.config.flag("ui", "cursor_indicator"));
}
#[test]
fn remote_recording_requires_owner_and_ignores_hotkey_release() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Recording, None);
    d.jobs.active.get_mut(&id).unwrap().owner = Some(7);
    assert!(d
        .command(
            8,
            &json!({
            "cmd":"stop_recording"}
            )
        )
        .is_err());
    d.event(Event::Key(KeyEvent::Up));
    assert_eq!(d.jobs.recording(), Some(id));
    d.command(
        7,
        &json!({
        "cmd":"stop_recording"}
        ),
    )
    .unwrap();
    assert!(d.jobs.recording().is_none());
    assert_eq!(d.jobs.active[&id].stage, Stage::Finalizing);
}
#[test]
fn remote_disconnect_finalizes_only_its_recording() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Recording, None);
    d.jobs.active.get_mut(&id).unwrap().owner = Some(7);
    let other = d.seed(Stage::Transcribing, None);
    d.event(Event::Disconnect(8));
    assert_eq!(d.jobs.recording(), Some(id));
    d.event(Event::Disconnect(7));
    assert!(d.jobs.recording().is_none());
    assert!(d.jobs.active.contains_key(&other));
    assert_eq!(d.jobs.active[&id].stage, Stage::Finalizing);
    assert!(!d.jobs.active[&id].cancelled.load(Ordering::Acquire));
}
#[test]
fn remote_start_respects_pause_and_existing_recording() {
    let (_root, mut d) = isolated();
    let (send, _receive) = mpsc::sync_channel(64);
    d.clients.insert(7, send);
    let request = json!({
    "cmd":"start_recording","pipewire_node":"synthetic"}
    );
    d.jobs.listening = false;
    assert!(d.command(7, &request).is_err());
    d.jobs.listening = true;
    d.seed(Stage::Recording, None);
    assert!(d.command(7, &request).is_err());
    assert!(!d.config.path.exists());
}
#[test]
fn queue_is_bounded_and_output_blocks_new_capture() {
    let (_root, mut d) = isolated();
    for _ in 0..4 {
        d.seed(Stage::Transcribing, None);
    }
    assert!(!d.jobs.can_record());
    d.cancel();
    assert!(d.jobs.can_record());
    d.seed(Stage::Delivering, None);
    assert!(!d.jobs.can_record());
}
#[test]
fn cancellation_keeps_inflight_output_owned_until_acknowledged() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Delivering, None);
    let cancelled = d.seed(Stage::Transcribing, None);
    let token = d.jobs.active[&cancelled].cancelled.clone();
    d.cancel();
    assert!(token.load(Ordering::Acquire));
    assert!(d.jobs.outputting());
    d.event(Event::Delivered(id, Ok(())));
    assert!(!d.jobs.busy());
}
#[test]
fn failed_load_records_correct_model_and_elapsed_time() {
    let (_root, mut d) = isolated();
    let row = d
        .history
        .add_recording("", std::path::Path::new("synthetic.wav"), 5., true)
        .unwrap();
    let config = d
        .config
        .changed(&json!({
        "transcription":{
        "model":"/nonexistent/synthetic-model.gguf"}
        }
        ))
        .unwrap();
    let id = d
        .jobs
        .insert(config.clone(), Stage::Transcribing, None, Some(row));
    d.worker
        .send(Work::Transcribe {
            id,
            generation: 0,
            cancelled: d.jobs.active[&id].cancelled.clone(),
            path: "synthetic.wav".into(),
            config,
            requested: Instant::now() - Duration::from_millis(200),
        })
        .unwrap();
    d.pump(|d| !d.jobs.busy());
    let row = d.history.get(row).unwrap().unwrap();
    assert_eq!(row["model"], "/nonexistent/synthetic-model.gguf");
    assert!(row["transcription_seconds"].as_f64().unwrap() >= 0.2);
    assert_eq!(row["failed"], true);
}
#[test]
fn stream_load_failure_finishes_without_waiting_for_audio_sender() {
    let (_root, mut d) = isolated();
    let config = d
        .config
        .changed(&json!({
        "transcription":{
        "model":"/nonexistent/synthetic-model.gguf"}
        }
        ))
        .unwrap();
    let id = d
        .jobs
        .insert(config.clone(), Stage::Transcribing, None, None);
    let (_audio, receive) = mpsc::sync_channel(10);
    d.worker
        .send(Work::Stream {
            id,
            generation: 0,
            cancelled: d.jobs.active[&id].cancelled.clone(),
            config,
            receive,
            finished: Arc::new(OnceLock::new()),
        })
        .unwrap();
    d.pump(|d| !d.jobs.busy());
    assert!(d.last_error.contains("does not exist"));
}
#[test]
fn stale_and_duplicate_results_cannot_touch_new_capture() {
    let (_root, mut d) = isolated();
    let old = d.seed(Stage::Transcribing, None);
    d.cancel();
    let new = d.seed(Stage::Recording, None);
    d.result(old, Err("stale failure"));
    d.result(old, Ok("stale preview"));
    assert_eq!(d.jobs.recording(), Some(new));
    assert!(d.last_error.is_empty());
}
#[test]
fn overlapping_results_keep_their_own_history() {
    let (_root, mut d) = isolated();
    let first = d
        .history
        .add_recording("", std::path::Path::new("one.wav"), 1., true)
        .unwrap();
    let second = d
        .history
        .add_recording("", std::path::Path::new("two.wav"), 2., true)
        .unwrap();
    let a = d.seed(Stage::Transcribing, Some(first));
    let b = d.seed(Stage::Transcribing, Some(second));
    d.result(a, Err("first failure"));
    d.pump(|d| d.io_pending == 0);
    d.result(a, Err("duplicate"));
    assert!(d.jobs.active.contains_key(&b));
    d.result(b, Ok("second words"));
    d.pump(|d| !d.jobs.busy());
    assert_eq!(d.history.get(first).unwrap().unwrap()["failed"], true);
    assert_eq!(
        d.history.get(second).unwrap().unwrap()["text"],
        "second words"
    );
}
#[test]
fn duplicate_history_retry_is_admitted_once() {
    let (root, mut d) = isolated();
    let path = root.path().join("synthetic.wav");
    std::fs::write(&path, []).unwrap();
    let row = d.history.add_recording("", &path, 1., true).unwrap();
    let (worker, receive) = mpsc::sync_channel(5);
    d.worker = worker;
    for _ in 0..100 {
        d.test_command(
            1,
            &json!({
            "cmd":"retry_history","id":row}
            ),
        )
        .unwrap();
    }
    let Work::Transcribe { id, .. } = receive.try_recv().unwrap() else {
        panic!("Expected retry")
    };
    assert!(receive.try_recv().is_err());
    assert_eq!(d.jobs.pending(), 1);
    d.result(id, Ok("retry words"));
    d.pump(|d| !d.jobs.busy());
    d.test_command(
        1,
        &json!({
        "cmd":"retry_history","id":row}
        ),
    )
    .unwrap();
    assert!(receive.try_recv().is_ok());
}
#[test]
fn slow_storage_does_not_block_state_or_cancellation() {
    let (_root, mut d) = isolated();
    let (send, receive) = mpsc::channel();
    d.effect(move |_| {
        receive.recv().unwrap();
        Event::Reply(
            1,
            Ok(json!({
            "type":"done"}
            )),
        )
    })
    .unwrap();
    let (client, replies) = mpsc::sync_channel(10);
    d.clients.insert(1, client);
    let start = Instant::now();
    d.command(
        1,
        &json!({
        "cmd":"get_state"}
        ),
    )
    .unwrap();
    assert_eq!(replies.try_recv().unwrap()["type"], "state");
    d.cancel();
    assert!(start.elapsed() < Duration::from_millis(100));
    send.send(()).unwrap();
    d.pump(|d| d.io_pending == 0);
}
#[test]
fn capture_failure_cannot_be_overwritten_by_late_stream_result() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Finalizing, None);
    d.event(Event::Finalized(
        id,
        Err(anyhow::anyhow!("microphone silence")),
    ));
    d.result(id, Err("decoder error"));
    assert!(d.last_error.contains("microphone silence"));
    assert!(!d.jobs.busy());
}
#[test]
fn failed_inference_waits_for_audio_persistence_before_terminal_state() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Recording, None);
    d.result(id, Err("synthetic engine error"));
    assert_eq!(d.jobs.active[&id].stage, Stage::Finalizing);
    assert!(d.last_error.is_empty());
    let row = d
        .history
        .add_recording("", std::path::Path::new("saved.wav"), 1., true)
        .unwrap();
    d.jobs
        .active
        .get_mut(&id)
        .unwrap()
        .transition(Stage::Saving);
    d.event(Event::Saved(
        id,
        Ok(SavedRecording {
            path: "saved.wav".into(),
            history_id: row,
            warning: None,
        }),
    ));
    d.pump(|d| !d.jobs.busy());
    assert_eq!(d.history.get(row).unwrap().unwrap()["failed"], true);
    assert!(d.last_error.contains("synthetic engine error"));
}
#[test]
fn prunes_old_and_oversized_recordings_but_keeps_favorites() {
    let (_root, mut daemon) = isolated();
    daemon.config = daemon
        .config
        .changed(&json!({"recordings":{"max_age_days":30,"max_mb":1}}))
        .unwrap();
    let directory = daemon.config.data_dir.join("recordings");
    std::fs::create_dir_all(&directory).unwrap();
    let write = |name: &str, days: u64| {
        let path = directory.join(name);
        std::fs::write(&path, vec![0u8; 600_000]).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(days * 86400);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(old).unwrap();
        path
    };
    let old = write("old.wav", 40);
    let favorite = write("favorite.wav", 40);
    let fresh = write("fresh.wav", 0);
    let extra = write("extra.wav", 20);
    let id = daemon
        .history
        .add_recording("kept", &favorite, 1., false)
        .unwrap();
    daemon.history.set_favorite(id, true).unwrap();
    daemon
        .history
        .add_recording("old", &old, 1., false)
        .unwrap();
    daemon
        .history
        .add_recording("fresh", &fresh, 1., false)
        .unwrap();
    crate::storage::prune_recordings(&daemon.history, &daemon.config, None).unwrap();
    assert!(!old.exists());
    assert!(favorite.exists());
    assert!(fresh.exists());
    assert!(extra.exists()); // Unindexed recordings are always protected.
    // Disabled limits leave files alone.
    daemon.config = daemon
        .config
        .changed(&json!({"recordings":{"max_age_days":0,"max_mb":0}}))
        .unwrap();
    let kept = write("kept.wav", 90);
    crate::storage::prune_recordings(&daemon.history, &daemon.config, None).unwrap();
    assert!(kept.exists());
}
#[test]
fn asynchronous_replies_keep_each_request_id() {
    let (_root, mut d) = isolated();
    let (send, receive) = mpsc::sync_channel(10);
    d.clients.insert(1, send);
    d.command(1, &json!({"cmd":"history","search":"one","request_id":17}))
        .unwrap();
    d.command(1, &json!({"cmd":"history","search":"two","request_id":18}))
        .unwrap();
    d.pump(|d| d.io_pending == 0);
    let first = receive.try_recv().unwrap();
    let second = receive.try_recv().unwrap();
    assert_eq!(first["request_id"], 17);
    assert_eq!(first["search"], "one");
    assert_eq!(second["request_id"], 18);
    assert_eq!(second["search"], "two");
}
#[test]
fn cancellation_before_queued_persistence_does_not_commit_text() {
    let (_root, mut d) = isolated();
    let (send, receive) = mpsc::channel();
    d.effect(move |_| {
        receive.recv().unwrap();
        Event::Reply(0, Ok(json!({})))
    })
    .unwrap();
    let id = d.seed(Stage::Transcribing, None);
    d.result(id, Ok("cancelled before save"));
    d.cancel();
    send.send(()).unwrap();
    d.pump(|d| d.io_pending == 0);
    assert!(d.history.recent(100, "").unwrap().is_empty());
    assert!(!d.jobs.busy());
}

#[test]
fn late_finalization_after_cancel_is_saved_without_output() {
    let (_root, mut d) = isolated();
    let id = d.seed(Stage::Finalizing, None);
    let directory = d.config.data_dir.join("recordings");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("dictation-cancelled.wav");
    let mut writer = hound::WavWriter::create(&path, hound::WavSpec {
        channels: 1, sample_rate: 16000, bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }).unwrap();
    for _ in 0..1600 { writer.write_sample(100_i16).unwrap(); }
    writer.finalize().unwrap();
    d.cancel();
    d.event(Event::Finalized(id, Ok((path.clone(), None))));
    d.pump(|d| d.io_pending == 0);
    assert!(path.exists());
    let rows = d.history.recent(10, "").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["failed"], true);
    assert!(!d.jobs.busy());
}
