use super::*;
pub(super) fn start_worker(
    receive: mpsc::Receiver<Work>,
    events: mpsc::SyncSender<Event>,
    current: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut engine: Option<crate::inference::Inference> = None;
        while let Ok(work) = receive.recv() {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            if let Work::Benchmark(config, durations, cancel) = work {
                engine = None;
                let result = optimization::run(&config, durations.as_slice(), &cancel, |value| {
                    let _ = events.send(Event::Benchmark(value));
                });
                let _ = events.send(Event::BenchmarkDone(result));
                continue;
            }
            let job_id = match &work {
                Work::Transcribe { id, .. } | Work::Stream { id, .. } => Some(*id),
                _ => None,
            };
            let _context = crate::logging::context(json!({"job_id":job_id}));
            let (config, generation, cancellation) = match &work {
                Work::Load(c) => (
                    c,
                    current.load(Ordering::Acquire),
                    Arc::new(AtomicBool::new(false)),
                ),
                Work::Transcribe {
                    config,
                    generation,
                    cancelled,
                    ..
                }
                | Work::Stream {
                    config,
                    generation,
                    cancelled,
                    ..
                } => (config, *generation, cancelled.clone()),
                Work::Benchmark(..) => unreachable!(),
            };
            let cancelled = || {
                shutdown.load(Ordering::Acquire)
                    || generation != current.load(Ordering::Acquire)
                    || cancellation.load(Ordering::Acquire)
            };
            if cancelled() {
                continue;
            }
            let identity = optimization::identity(config);
            let model = identity.0.clone();
            let loaded = if engine.as_ref().is_some_and(|e| e.matches(config)) {
                Ok(())
            } else {
                engine = None;
                let _ = events.send(Event::ModelLoading(identity.clone()));
                log::info!("Loading speech model: {} / {}", identity.0, identity.1);
                crate::inference::Inference::load(config, cancelled).map(|loaded| {
                    engine = Some(loaded);
                })
            };
            if let Err(error) = loaded {
                let _ = events.send(Event::WorkerUnavailable(identity.clone()));
                let detail = format!("{error:#}");
                match work {
                    Work::Load(_) => {
                        let _ = events.send(Event::Model(identity, Err(error)));
                    }
                    Work::Transcribe { id, requested, .. } => {
                        let _ = events.send(Event::WorkerTranscribed(
                            id,
                            cancellation.clone(),
                            Transcript {
                                text: Err(detail),
                                seconds: requested.elapsed().as_secs_f64(),
                                model,
                            },
                        ));
                    }
                    Work::Stream { id, finished, .. } => {
                        let _ = events.send(Event::WorkerTranscribed(
                            id,
                            cancellation.clone(),
                            Transcript {
                                text: Err(detail),
                                seconds: finished
                                    .get()
                                    .map(|s| s.elapsed().as_secs_f64())
                                    .unwrap_or(0.),
                                model,
                            },
                        ));
                    }
                    Work::Benchmark(..) => unreachable!(),
                }
                continue;
            }
            let _ = events.send(Event::Model(
                identity.clone(),
                Ok(engine.as_ref().unwrap().profile.clone()),
            ));
            let (id, result, seconds) = match work {
                Work::Load(_) => continue,
                Work::Transcribe {
                    id,
                    path,
                    config,
                    requested,
                    ..
                } => {
                    let result = engine
                        .as_mut()
                        .unwrap()
                        .transcribe(path, &config, cancelled);
                    (id, result, requested.elapsed().as_secs_f64())
                }
                Work::Stream {
                    id,
                    receive,
                    finished,
                    ..
                } => {
                    let result = engine
                        .as_mut()
                        .unwrap()
                        .stream(&receive, cancelled, |committed, tentative| {
                            let _ = events.try_send(Event::Preview(id, committed, tentative));
                        })
                        .and_then(|text| {
                            text.ok_or_else(|| anyhow::anyhow!("Streaming cancelled"))
                        });
                    (
                        id,
                        result,
                        finished
                            .get()
                            .map(|s| s.elapsed().as_secs_f64())
                            .unwrap_or(0.),
                    )
                }
                Work::Benchmark(..) => unreachable!(),
            };
            if let Err(error) = &result {
                if cancelled() {
                    log::warn!("Inference attempt cancelled by coordinator: job={id} generation={generation} current_generation={} shutdown={} error={error:#}", current.load(Ordering::Acquire), shutdown.load(Ordering::Acquire));
                } else {
                    log::error!(
                        "Inference operation failed: job={id} model={model} error={error:#}"
                    );
                }
                engine = None;
                let _ = events.send(Event::WorkerUnavailable(identity));
            }
            let _ = events.send(Event::WorkerTranscribed(
                id,
                cancellation.clone(),
                Transcript {
                    text: result.map_err(|e| format!("{e:#}")),
                    seconds,
                    model,
                },
            ));
        }
    });
}
