use crate::{config::Config, process};
use anyhow::{bail, Result};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
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
}
impl Capture {
    pub fn exited(&mut self) -> bool {
        self.child.try_wait().is_ok_and(|status| status.is_some())
    }
    pub fn start(c: &Config, callback: impl Fn(f32, f32) + Send + 'static) -> Result<Self> {
        Self::start_command(c, callback, Command::new("arecord"))
    }
    fn start_command(
        c: &Config,
        callback: impl Fn(f32, f32) + Send + 'static,
        mut command: Command,
    ) -> Result<Self> {
        let template = crate::config::expand(c.string("audio", "temp_file"));
        let directory = template.parent().unwrap_or(std::path::Path::new("/tmp"));
        std::fs::create_dir_all(directory)?;
        let file = tempfile::Builder::new()
            .prefix("dictation-")
            .suffix(".wav")
            .tempfile_in(directory)?;
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
        let child = command.spawn()?;
        log::info!(
            "Capture started: rate={} channels={}",
            c.number("audio", "sample_rate"),
            c.number("audio", "channels")
        );
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let path = file.path().to_owned();
        let monitor = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                if let Ok(mut file) = File::open(&path) {
                    if let Ok(meta) = file.metadata() {
                        if meta.len() > 128 {
                            let _ = file
                                .seek(SeekFrom::Start(meta.len().saturating_sub(3200).max(128)));
                            let mut bytes = Vec::new();
                            let _ = file.take(3200).read_to_end(&mut bytes);
                            let (level, db) = level(&bytes);
                            callback(level, db);
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
        })
    }
    pub fn finish(mut self) -> Result<NamedTempFile> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.monitor.take() {
            let _ = thread.join();
        }
        if self.child.try_wait()?.is_some() {
            self.error.rewind()?;
            let mut message = String::new();
            self.error
                .by_ref()
                .take(4096)
                .read_to_string(&mut message)?;
            bail!("Audio capture failed: {message}")
        }
        // SAFETY: this is the live child PID, SIGINT lets arecord finalize its WAV header.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGINT);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait()?.is_none() {
            if Instant::now() > deadline {
                bail!("Audio capture did not stop cleanly")
            }
            thread::sleep(Duration::from_millis(10));
        }
        let file = self.file.take().unwrap();
        let mut wav = hound::WavReader::open(file.path())?;
        if wav.duration() < wav.spec().sample_rate / 10 {
            bail!("Recording was too short")
        }
        if wav.spec().bits_per_sample == 16
            && !wav.samples::<i16>().any(|s| s.is_ok_and(|v| v != 0))
        {
            bail!("Microphone returned silence. Check the microphone and mute setting.")
        }
        Ok(file)
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
        let capture = Capture::start_command(c, |_, _| {}, command).unwrap();
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
    fn missing_recorder_cleans_up_private_file() {
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path());
        assert!(Capture::start_command(
            &c,
            |_, _| {},
            Command::new(root.path().join("missing-recorder"))
        )
        .is_err());
        assert_eq!(
            std::fs::read_dir(root.path().join("recordings"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn concurrent_recordings_have_private_distinct_files() {
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
        let file = first.finish().unwrap();
        assert!(file.path().exists());
        drop(file);
        drop(second);
        assert_eq!(
            std::fs::read_dir(root.path().join("recordings"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn silent_capture_is_rejected_before_transcription() {
        let root = tempfile::tempdir().unwrap();
        let c = config(root.path());
        assert!(synthetic(&c, 0)
            .finish()
            .unwrap_err()
            .to_string()
            .contains("silence"));
        assert_eq!(
            std::fs::read_dir(root.path().join("recordings"))
                .unwrap()
                .count(),
            0
        );
    }
}
