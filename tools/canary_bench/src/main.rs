use anyhow::{bail, Result};
use serde_json::json;
use std::{io::Write, path::Path, time::Instant};
use transcribe_rs::{
    onnx::{canary::CanaryModel, Quantization},
    SpeechModel, TranscribeOptions,
};

// Only one resident engine exists per benchmark process; keep Handy's model inline.
#[allow(clippy::large_enum_variant)]
enum Engine {
    Gguf(transcribe_cpp::Session),
    Onnx(CanaryModel),
}
impl Engine {
    fn load(backend: &str, path: &Path) -> Result<Self> {
        Ok(match backend {
            "gguf" => Self::Gguf(
                transcribe_cpp::Model::load_with(
                    path,
                    &transcribe_cpp::ModelOptions {
                        backend: transcribe_cpp::Backend::Cpu,
                        device: None,
                    },
                )?
                .session()?,
            ),
            "onnx" => Self::Onnx(CanaryModel::load(path, &Quantization::Int8)?),
            _ => bail!("Expected gguf or onnx"),
        })
    }
    fn run(&mut self, samples: &[f32]) -> Result<String> {
        Ok(match self {
            Self::Gguf(session) => {
                session
                    .run(
                        samples,
                        &transcribe_cpp::RunOptions {
                            language: Some("en".into()),
                            ..Default::default()
                        },
                    )?
                    .text
            }
            Self::Onnx(model) => {
                model
                    .transcribe(
                        samples,
                        &TranscribeOptions {
                            language: Some("en".into()),
                            ..Default::default()
                        },
                    )?
                    .text
            }
        })
    }
}
fn chunk_end(samples: &[f32]) -> usize {
    if samples.len() <= 480000 {
        return samples.len();
    }
    let (frame, _) = samples[400000..480000]
        .chunks_exact(320)
        .enumerate()
        .map(|(i, f)| (i, f.iter().map(|v| v * v).sum::<f32>()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    400000 + frame * 320 + 160
}
fn usage() -> (f64, i64) {
    let mut r = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    unsafe {
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, r.as_mut_ptr()), 0);
        let r = r.assume_init();
        (
            r.ru_utime.tv_sec as f64
                + r.ru_stime.tv_sec as f64
                + (r.ru_utime.tv_usec + r.ru_stime.tv_usec) as f64 / 1e6,
            r.ru_maxrss,
        )
    }
}
fn emit(v: serde_json::Value) {
    println!("{v}");
    std::io::stdout().flush().unwrap();
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 5 {
        bail!("backend model rounds audio.f32...");
    }
    let audio = args[4..]
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path)?;
            Ok((
                path.clone(),
                bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let started = Instant::now();
    let mut engine = Engine::load(&args[1], Path::new(&args[2]))?;
    emit(
        json!({"event":"loaded", "backend":args[1], "seconds":started.elapsed().as_secs_f64(), "peak_rss_kib":usage().1}),
    );
    for round in 0..args[3].parse::<usize>()? {
        for (path, samples) in &audio {
            let start = Instant::now();
            let cpu = usage().0;
            let result = (|| -> Result<String> {
                let mut rest = samples.as_slice();
                let mut text = Vec::new();
                while !rest.is_empty() {
                    let end = chunk_end(rest);
                    text.push(engine.run(&rest[..end])?);
                    rest = &rest[end..];
                }
                Ok(text.join(" "))
            })();
            let elapsed = start.elapsed().as_secs_f64();
            let (used, rss) = usage();
            emit(
                json!({"event":"transcribed", "backend":args[1], "round":round, "audio":path,
                "audio_seconds":samples.len() as f64/16000., "seconds":elapsed, "cpu_seconds":used-cpu,
                "peak_rss_kib":rss, "text":result.as_ref().ok(), "error":result.err().map(|e| e.to_string())}),
            );
        }
    }
    let cpu = usage().0;
    std::thread::sleep(std::time::Duration::from_secs(2));
    emit(json!({"event":"idle", "cpu_seconds_over_2s":usage().0-cpu}));
    Ok(())
}
