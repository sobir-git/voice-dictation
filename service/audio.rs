use crate::{config::Config, process};
use anyhow::Result;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

fn wav_data_offset(file: &mut File) -> Option<u64> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut header = vec![0; file.metadata().ok()?.len().min(65_536) as usize];
    file.read_exact(&mut header).ok()?;
    if header.get(0..4)? != b"RIFF" || header.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut position = 12_usize;
    while position.checked_add(8)? <= header.len() {
        let size = u32::from_le_bytes(header[position + 4..position + 8].try_into().ok()?) as usize;
        if &header[position..position + 4] == b"data" {
            return Some((position + 8) as u64);
        }
        position = position.checked_add(8 + size + (size & 1))?;
    }
    None
}

/// Repair sizes left by an interrupted WAV writer without replacing audio bytes.
/// Unknown or incomplete headers remain on disk for manual recovery.
pub fn repair_wav(path: &std::path::Path) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let offset = wav_data_offset(&mut file).ok_or_else(|| {
        anyhow::anyhow!("Incomplete WAV header; recovery audio: {}", path.display())
    })?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(offset - 4))?;
    let mut size = [0; 4];
    file.read_exact(&mut size)?;
    let declared = u32::from_le_bytes(size) as u64;
    let available = length.saturating_sub(offset);
    // Our recorder writes data as the final chunk. A killed writer may leave
    // either an oversized placeholder or a header describing only its first block.
    if declared != available {
        // hound reads the format even when the declared data length is stale.
        let reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        let frame = u64::from(spec.channels) * u64::from(spec.bits_per_sample).div_ceil(8);
        anyhow::ensure!(frame > 0, "Invalid WAV format at {}", path.display());
        let data_size = u32::try_from(available - available % frame)?;
        file.seek(SeekFrom::Start(offset - 4))?;
        file.write_all(&data_size.to_le_bytes())?;
        file.seek(SeekFrom::Start(4))?;
        file.write_all(&u32::try_from(length - 8)?.to_le_bytes())?;
        file.sync_all()?;
    }
    Ok(())
}

pub fn level(bytes: &[u8]) -> (f32, f32) {
    let n = bytes.len() / 2;
    let energy = bytes
        .chunks_exact(2)
        .map(|b| {
            let x = i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.;
            x * x
        })
        .sum::<f64>();
    let db = (20. * (energy / n.max(1) as f64).sqrt().max(1e-6).log10()) as f32;
    (((db + 60.) / 60.).clamp(0., 1.), db)
}
pub struct Capture {
    child: Child,
    file: Option<NamedTempFile>,
    error: File,
    stop: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
    spawn_owner: Option<std::sync::mpsc::Sender<()>>,
}
impl Capture {
    /// Linux PDEATHSIG tracks the spawning thread. Keep asynchronous startup's
    /// thread alive until this capture has stopped and reaped its recorder.
    pub fn retain_spawn_owner(&mut self, owner: std::sync::mpsc::Sender<()>) {
        self.spawn_owner = Some(owner);
    }

    pub fn exited(&mut self) -> bool {
        self.child.try_wait().is_ok_and(|status| status.is_some())
    }
    pub fn start(
        c: &Config,
        callback: impl Fn(Vec<f32>, f32, f32) + Send + 'static,
    ) -> Result<Self> {
        Self::start_command(c, callback, Command::new("arecord"))
    }
    fn start_command(
        c: &Config,
        callback: impl Fn(Vec<f32>, f32, f32) + Send + 'static,
        mut command: Command,
    ) -> Result<Self> {
        // Capture directly into persistent storage, before the recorder can write.
        // A dropped job, panic or SIGKILL must never unlink its only audio.
        let directory = c.data_dir.join("recordings");
        std::fs::create_dir_all(&directory)?;
        let file = tempfile::Builder::new()
            .prefix("dictation-")
            .suffix(".wav")
            .disable_cleanup(true)
            .tempfile_in(&directory)?;
        File::open(&directory)?.sync_all()?;
        let error = tempfile::tempfile()?;
        command
            .args([
                "-q",
                "-D",
                c.string("audio", "device"),
                "-f",
                c.string("audio", "format"),
                "-r",
                &c.number("audio", "sample_rate").to_string(),
                "-c",
                &c.number("audio", "channels").to_string(),
            ])
            .arg(file.path())
            .stdout(Stdio::null())
            .stderr(error.try_clone()?)
            .stdin(Stdio::null());
        if !c.string("audio", "pipewire_node").is_empty() {
            command.env("PIPEWIRE_NODE", c.string("audio", "pipewire_node"));
        }
        process::kill_with_parent(&mut command);
        let child = command.spawn().inspect_err(|_| {
            // No recorder was spawned, so this file cannot contain captured audio.
            let _ = std::fs::remove_file(file.path());
        })?;
        log::info!("Capture recovery audio: {}", file.path().display());
        log::info!(
            "Capture started: rate={} channels={}",
            c.number("audio", "sample_rate"),
            c.number("audio", "channels")
        );
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let path = file.path().to_owned();
        let monitor = thread::spawn(move || {
            let mut offset = None;
            let mut synced = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                if let Ok(mut file) = File::open(&path) {
                    if synced.elapsed() >= Duration::from_secs(1) {
                        if let Err(error) = file.sync_data() {
                            log::error!("Could not sync capture at {}: {error}", path.display());
                        }
                        synced = Instant::now();
                    }
                    if let Ok(meta) = file.metadata() {
                        let Some(start) = offset.or_else(|| wav_data_offset(&mut file)) else {
                            continue;
                        };
                        offset = Some(start);
                        let end = meta.len() - meta.len() % 2;
                        if end > start && file.seek(SeekFrom::Start(start)).is_ok() {
                            let mut bytes = Vec::with_capacity((end - start) as usize);
                            let _ = file.take(end - start).read_to_end(&mut bytes);
                            offset = Some(start + bytes.len() as u64);
                            let (level, db) = level(&bytes);
                            let samples = bytes
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.)
                                .collect();
                            callback(samples, level, db);
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            file: Some(file),
            error,
            stop,
            monitor: Some(monitor),
            spawn_owner: None,
        })
    }
    /// Finalization always returns ownership of the retained audio, including
    /// when the recorder or WAV validation fails. Failure is a warning, not a
    /// reason to drop the only recovery path.
    pub fn finish(mut self) -> (NamedTempFile, Option<String>) {
        let warning = match self.finalize() {
            Ok(warning) => warning,
            Err(error) => {
                self.stop.store(true, Ordering::Relaxed);
                let _ = self.child.kill();
                let _ = self.child.wait();
                if let Some(thread) = self.monitor.take() {
                    let _ = thread.join();
                }
                let path = self.file.as_ref().unwrap().path();
                let _ = repair_wav(path);
                log::error!(
                    "Capture finalization failed; recovery audio: {}: {error:#}",
                    path.display()
                );
                Some(format!(
                    "Capture finalization failed: {error:#}. Audio was retained."
                ))
            }
        };
        (self.file.take().unwrap(), warning)
    }
    fn finalize(&mut self) -> Result<Option<String>> {
        let mut forced = false;
        let mut interruption = if self.child.try_wait()?.is_some() {
            self.error.rewind()?;
            let mut message = String::new();
            std::io::Read::by_ref(&mut self.error)
                .take(4096)
                .read_to_string(&mut message)?;
            Some(if message.trim().is_empty() {
                "The microphone stopped unexpectedly.".to_owned()
            } else {
                format!("The microphone stopped unexpectedly: {}", message.trim())
            })
        } else {
            // SAFETY: this is the live child PID, SIGINT lets arecord finalize its WAV header.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGINT);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.child.try_wait()?.is_none() {
                if Instant::now() > deadline {
                    forced = true;
                    self.child.kill()?;
                    self.child.wait()?;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            None
        };
        if forced {
            interruption =
                Some("Recorder did not stop cleanly; captured audio was retained.".into());
        }
        // Let the monitor consume the recorder's final buffered samples before
        // closing the live stream. Its current iteration reads once after this
        // flag changes, so the finalized WAV tail is forwarded exactly once.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.monitor.take() {
            let _ = thread.join();
        }
        let file = self.file.as_ref().unwrap();
        repair_wav(file.path())?;
        file.as_file().sync_all()?;
        let mut wav = hound::WavReader::open(file.path())?;
        if wav.duration() < wav.spec().sample_rate / 10 {
            interruption.get_or_insert_with(|| {
                "Recording was too short; captured audio was retained.".into()
            });
        }
        if wav.spec().bits_per_sample == 16 {
            let mut voiced = false;
            for sample in wav.samples::<i16>() {
                if sample? != 0 {
                    voiced = true;
                    break;
                }
            }
            if !voiced {
                interruption.get_or_insert_with(|| {
                    "Microphone returned silence. Check the microphone and mute setting.".into()
                });
            }
        }
        Ok(interruption)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.monitor.take() {
            let _ = thread.join();
        }
    }
}

pub fn microphones() -> serde_json::Value {
    let Ok(bytes) = process::run(
        Command::new("pactl").args(["-f", "json", "list", "sources"]),
        None,
        Duration::from_secs(3),
    ) else {
        return serde_json::json!([]);
    };
    let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) else {
        return serde_json::json!([]);
    };
    serde_json::Value::Array(rows.into_iter()
        .filter(|row| !row["name"].as_str().unwrap_or("").ends_with(".monitor"))
        .map(|row| serde_json::json!({"name":row["name"], "description":row["description"], "muted":row["mute"]}))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(root: &std::path::Path) -> Config {
        Config::at(root)
            .unwrap()
            .changed(
                &serde_json::json!({"audio":{"temp_file":root.join("recordings/capture.wav")}}),
            )
            .unwrap()
    }
    fn synthetic(c: &Config, signal: i16) -> Capture {
        let fixture = c
            .path
            .parent()
            .unwrap()
            .join(format!("fixture-{signal}.wav"));
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        let mut wav = hound::WavWriter::create(
            &fixture,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..4000 {
            wav.write_sample(signal).unwrap();
        }
        wav.finalize().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "trap 'exit 0' INT; for destination do :; done; cp \"$STT_FIXTURE\" \"$destination\"; while :; do sleep 0.05; done", "capture"])
            .env("STT_FIXTURE", fixture);
        let capture = Capture::start_command(c, |_, _, _| {}, command).unwrap();
        for _ in 0..100 {
            if capture
                .file
                .as_ref()
                .unwrap()
                .as_file()
                .metadata()
                .unwrap()
                .len()
                > 128
            {
                return capture;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Synthetic capture did not write its WAV")
    }
    #[test]
    fn finds_audio_after_optional_wav_chunks() {
        let mut file = tempfile::tempfile().unwrap();
        use std::io::Write;
        file.write_all(b"RIFF\x1c\0\0\0WAVEJUNK\x03\0\0\0abc\0data\x02\0\0\0\x01\0")
            .unwrap();
        assert_eq!(wav_data_offset(&mut file), Some(32));
    }
    #[test]
    fn missing_recorder_cleans_up_private_file() {
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path());
        assert!(Capture::start_command(
            &c,
            |_, _, _| {},
            Command::new(root.path().join("missing-recorder"))
        )
        .is_err());
        assert_eq!(
            std::fs::read_dir(c.data_dir.join("recordings"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn concurrent_recordings_have_private_distinct_retained_files() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path());
        let first = synthetic(&c, 100);
        let second = synthetic(&c, 200);
        assert_ne!(
            first.file.as_ref().unwrap().path(),
            second.file.as_ref().unwrap().path()
        );
        assert_eq!(
            first
                .file
                .as_ref()
                .unwrap()
                .as_file()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let (file, interruption) = first.finish();
        assert!(interruption.is_none());
        assert!(file.path().exists());
        drop(file);
        drop(second);
        assert_eq!(
            std::fs::read_dir(c.data_dir.join("recordings"))
                .unwrap()
                .count(),
            2
        );
    }
    #[test]
    fn silent_capture_is_retained_with_warning() {
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path());
        let (file, warning) = synthetic(&c, 0).finish();
        assert!(warning.unwrap().contains("silence"));
        let path = file.path().to_owned();
        drop(file);
        assert!(path.exists());
    }
    #[test]
    fn unexpected_recorder_exit_preserves_valid_audio() {
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path());
        let fixture = root.path().join("fixture.wav");
        let mut wav = hound::WavWriter::create(
            &fixture,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..4000 {
            wav.write_sample(100_i16).unwrap();
        }
        wav.finalize().unwrap();
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "for destination do :; done; cp \"$STT_FIXTURE\" \"$destination\"; echo 'device disconnected' >&2; exit 1",
                "capture",
            ])
            .env("STT_FIXTURE", fixture);
        let mut capture = Capture::start_command(&c, |_, _, _| {}, command).unwrap();
        for _ in 0..100 {
            if capture.exited() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(capture.exited());
        let (file, interruption) = capture.finish();
        assert!(file.path().exists());
        assert!(interruption.unwrap().contains("device disconnected"));
    }
}
