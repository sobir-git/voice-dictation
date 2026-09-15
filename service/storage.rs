//! One owner for SQLite and disk effects. The coordinator only enqueues work.
use crate::{daemon::Event, history::History};
use anyhow::{anyhow, Result};
use std::{sync::mpsc, thread};
type Task = (Option<u64>, Box<dyn FnOnce(&History) -> Event + Send>);
pub struct Storage {
    send: mpsc::Sender<Task>,
}
impl Storage {
    pub fn start(history: History, events: mpsc::SyncSender<Event>) -> Self {
        let (send, receive) = mpsc::channel::<Task>();
        thread::spawn(move || {
            while let Ok((request_id, task)) = receive.recv() {
                let _context = crate::logging::context(
                    serde_json::json!({"request_id":request_id,"component":"storage"}),
                );
                if events
                    .send(Event::Storage(request_id, Box::new(task(&history))))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { send }
    }
    pub fn submit(
        &self,
        request_id: Option<u64>,
        task: impl FnOnce(&History) -> Event + Send + 'static,
    ) -> Result<()> {
        self.send
            .send((request_id, Box::new(task)))
            .map_err(|_| anyhow!("Storage worker is busy or unavailable"))
    }
}
pub struct SavedRecording {
    pub path: std::path::PathBuf,
    pub history_id: i64,
    pub warning: Option<String>,
}
pub fn save_recording(
    history: &History,
    directory: &std::path::Path,
    source: std::path::PathBuf,
    mut warning: Option<String>,
) -> Result<SavedRecording> {
    // Retain the original temporary audio if persistence fails; never destroy
    // the user's only recording on a full/unavailable destination filesystem.
    let result = (|| -> Result<SavedRecording> {
        std::fs::create_dir_all(directory)?;
        let path = if source.parent() == Some(directory) {
            // Capture already owns a durable file. Do not copy or rename it:
            // the same path identifies this recording across crash recovery.
            std::fs::File::open(&source)?.sync_all()?;
            source.clone()
        } else {
            let mut destination = tempfile::Builder::new()
                .prefix("dictation-")
                .suffix(".wav")
                .tempfile_in(directory)?;
            std::io::copy(&mut std::fs::File::open(&source)?, &mut destination)?;
            destination.as_file().sync_all()?;
            let (_, path) = destination.keep()?;
            std::fs::File::open(directory)?.sync_all()?;
            path
        };
        let duration = match hound::WavReader::open(&path) {
            Ok(reader) => reader.duration() as f64 / reader.spec().sample_rate.max(1) as f64,
            Err(error) => {
                warning = Some(format!(
                    "WAV is incomplete: {error}. Audio retained at {}",
                    path.display()
                ));
                0.0
            }
        };
        let history_id = match history.recording_id(&path)? {
            Some(id) => id,
            None => history
                .add_recording("", &path, duration, true)
                .map_err(|error| {
                    anyhow!(
                        "Recording is saved at {}; history write failed: {error}",
                        path.display()
                    )
                })?,
        };
        log::info!("Recording saved: history={history_id} audio_seconds={duration:.3}");
        Ok(SavedRecording {
            path,
            history_id,
            warning,
        })
    })();
    if result.as_ref().is_ok_and(|saved| saved.path != source) {
        let _ = std::fs::remove_file(&source);
    }
    result.map_err(|error| anyhow!("{error:#}; recovery audio: {}", source.display()))
}
/// Restore durable captures that never reached SQLite. Call before admitting
/// capture work, so an active writer cannot be mistaken for an interrupted one.
pub fn recover_recordings(history: &History, directory: &std::path::Path) -> Result<usize> {
    let known: std::collections::HashSet<String> = history
        .recordings()?
        .into_iter()
        .map(|(_, path, _)| path)
        .collect();
    if !directory.exists() {
        return Ok(0);
    }
    let mut recovered = 0;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || !entry
                .file_name()
                .to_string_lossy()
                .starts_with("dictation-")
            || path.extension().is_none_or(|extension| extension != "wav")
            || known.contains(&*path.to_string_lossy())
        {
            continue;
        }
        let warning = match crate::audio::repair_wav(&path) {
            Ok(()) => "Recovered interrupted recording".into(),
            Err(error) => format!("Recovered incomplete recording: {error:#}"),
        };
        let result = save_recording(history, directory, path.clone(), Some(warning));
        match result {
            Ok(saved) => {
                recovered += 1;
                log::warn!(
                    "Recovered recording: history={} audio={}",
                    saved.history_id,
                    path.display()
                );
            }
            Err(error) => log::error!(
                "Recording recovery failed; audio retained at {}: {error:#}",
                path.display()
            ),
        }
    }
    Ok(recovered)
}
pub fn prune_recordings(
    history: &History,
    config: &crate::config::Config,
    keep: Option<&std::path::Path>,
) -> Result<()> {
    let max_age_days = config.number("recordings", "max_age_days");
    let max_bytes = config.number("recordings", "max_mb") * 1024 * 1024;
    if max_age_days == 0 && max_bytes == 0 {
        return Ok(());
    }
    let directory = config.data_dir.join("recordings");
    let eligible: std::collections::HashSet<String> =
        history.prunable_recordings()?.into_iter().collect();
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            max_age_days.saturating_mul(86400),
        ))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut candidates: Vec<(std::path::PathBuf, std::time::SystemTime, u64)> =
        std::fs::read_dir(&directory)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let path = entry.path();
                        let meta = entry.metadata().ok()?;
                        (meta.is_file()
                            && path.extension().is_some_and(|e| e == "wav")
                            && Some(path.as_path()) != keep
                            && eligible.contains(&*path.to_string_lossy()))
                        .then(|| meta.modified().ok().map(|m| (path, m, meta.len())))
                        .flatten()
                    })
                    .collect()
            })
            .unwrap_or_default();
    candidates.sort_by_key(|(_, modified, _)| *modified);
    let mut total: u64 = candidates.iter().map(|(_, _, size)| size).sum();
    let mut removed = 0;
    for (path, modified, size) in candidates {
        let expired = max_age_days > 0 && modified < cutoff;
        let over = max_bytes > 0 && total > max_bytes;
        if !expired && !over {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("Pruned {removed} recordings over the retention limit");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn audio(root: &std::path::Path) -> std::path::PathBuf {
        let path = root.join("synthetic.wav");
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..1600 {
            writer.write_sample(100i16).unwrap();
        }
        writer.finalize().unwrap();
        path
    }
    #[test]
    fn failed_destination_preserves_original_audio() {
        let root = tempfile::tempdir().unwrap();
        let history = History::open(root.path()).unwrap();
        let source = audio(root.path());
        let destination = root.path().join("not-a-directory");
        std::fs::write(&destination, b"occupied").unwrap();
        let error = save_recording(&history, &destination, source.clone(), None)
            .err()
            .unwrap();
        assert!(source.exists());
        assert!(error.to_string().contains("recovery audio"));
        assert!(history.recent(100, "").unwrap().is_empty());
    }
    #[test]
    fn successful_save_keeps_audio_and_replaces_temporary_source() {
        let root = tempfile::tempdir().unwrap();
        let history = History::open(root.path()).unwrap();
        let source = audio(root.path());
        let saved = save_recording(
            &history,
            &root.path().join("recordings"),
            source.clone(),
            None,
        )
        .unwrap();
        assert!(saved.path.exists());
        assert!(!source.exists());
        assert!(history.get(saved.history_id).unwrap().is_some());
    }
    #[test]
    fn sqlite_failure_preserves_audio_for_later_recovery() {
        let root = tempfile::tempdir().unwrap();
        let history = History::open(root.path()).unwrap();
        let directory = root.path().join("recordings");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("dictation-history-failure.wav");
        std::fs::rename(audio(&directory), &path).unwrap();
        let db = rusqlite::Connection::open(root.path().join("history.db")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON history BEGIN SELECT RAISE(ABORT, 'synthetic disk failure'); END;").unwrap();
        assert!(save_recording(&history, &directory, path.clone(), None).is_err());
        assert!(path.exists());
        db.execute_batch("DROP TRIGGER fail_insert;").unwrap();
        assert_eq!(recover_recordings(&history, &directory).unwrap(), 1);
        assert_eq!(history.recent(10, "").unwrap().len(), 1);
    }
    #[test]
    fn duplicate_save_does_not_duplicate_history_or_delete_capture() {
        let root = tempfile::tempdir().unwrap();
        let history = History::open(root.path()).unwrap();
        let source = audio(root.path());
        let first = save_recording(&history, root.path(), source.clone(), None).unwrap();
        let second = save_recording(&history, root.path(), source.clone(), None).unwrap();
        assert_eq!(first.history_id, second.history_id);
        assert!(source.exists());
        assert_eq!(history.recent(10, "").unwrap().len(), 1);
    }
    #[test]
    fn crash_recovery_repairs_wav_and_is_idempotent() {
        use std::io::{Seek, Write};
        let root = tempfile::tempdir().unwrap();
        let history = History::open(root.path()).unwrap();
        let directory = root.path().join("recordings");
        std::fs::create_dir(&directory).unwrap();
        let source = audio(&directory);
        let path = directory.join("dictation-interrupted.wav");
        std::fs::rename(source, &path).unwrap();
        // arecord writes a provisional data length until it receives SIGINT.
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(std::io::SeekFrom::Start(40)).unwrap();
        file.write_all(&0x7fff_f000_u32.to_le_bytes()).unwrap();
        assert_eq!(recover_recordings(&history, &directory).unwrap(), 1);
        assert_eq!(recover_recordings(&history, &directory).unwrap(), 0);
        let rows = history.recent(10, "").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["failed"], true);
        assert_eq!(rows[0]["duration"], 0.1);
        assert!(path.exists());
        assert_eq!(
            hound::WavReader::open(path)
                .unwrap()
                .samples::<i16>()
                .count(),
            1600
        );
    }
    #[test]
    fn malformed_or_failed_audio_survives_recovery_and_retention() {
        let root = tempfile::tempdir().unwrap();
        let config = crate::config::Config::at(root.path())
            .unwrap()
            .changed(&serde_json::json!({"recordings":{"max_age_days":1,"max_mb":1}}))
            .unwrap();
        let history = History::open(&config.data_dir).unwrap();
        let directory = config.data_dir.join("recordings");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("dictation-incomplete.wav");
        std::fs::write(&path, b"RIFFpartial").unwrap();
        let failed = audio(&directory);
        history.add_recording("", &failed, 0.1, true).unwrap();
        for path in [&path, &failed] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(
                    std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86400),
                )
                .unwrap();
        }
        assert_eq!(recover_recordings(&history, &directory).unwrap(), 1);
        prune_recordings(&history, &config, None).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"RIFFpartial");
        assert!(failed.exists());
    }
    #[test]
    fn retention_is_disabled_by_default() {
        let root = tempfile::tempdir().unwrap();
        let config = crate::config::Config::at(root.path()).unwrap();
        assert_eq!(config.number("recordings", "max_age_days"), 0);
        assert_eq!(config.number("recordings", "max_mb"), 0);
    }
}
