use crate::{
    config::{expand, Config},
    optimization, process,
};
use anyhow::{bail, Context, Result};
use ct2rs::{
    sys::{StorageView, Whisper, WhisperOptions},
    ComputeType,
};
use rustfft::{num_complex::Complex32, FftPlanner};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::Duration,
};

pub const PARAKEET_MODEL: &str = "parakeet-unified-en-0.6b";
pub const CANARY_MODEL: &str = "canary-180m-flash";

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn omp_pause_resource_all(kind: i32) -> i32;
}

struct OpenMpPause;

#[cfg(target_os = "linux")]
struct CpuAffinity(libc::cpu_set_t);

#[cfg(target_os = "linux")]
impl CpuAffinity {
    fn performance_cores(enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        unsafe {
            let mut old: libc::cpu_set_t = std::mem::zeroed();
            if libc::sched_getaffinity(0, std::mem::size_of_val(&old), &mut old) != 0 {
                return None;
            }
            let mut frequencies = Vec::new();
            for cpu in 0..libc::CPU_SETSIZE as usize {
                if !libc::CPU_ISSET(cpu, &old) {
                    continue;
                }
                let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq");
                if let Ok(value) = std::fs::read_to_string(path) {
                    if let Ok(value) = value.trim().parse::<u64>() {
                        frequencies.push((cpu, value));
                    }
                }
            }
            let maximum = frequencies.iter().map(|(_, value)| *value).max()?;
            let selected: Vec<_> = frequencies
                .iter()
                .filter(|(_, value)| *value * 10 >= maximum * 9)
                .map(|(cpu, _)| *cpu)
                .collect();
            if selected.len() < 2 || selected.len() == frequencies.len() {
                return None;
            }
            let mut fast: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_ZERO(&mut fast);
            for cpu in selected {
                libc::CPU_SET(cpu, &mut fast);
            }
            if libc::sched_setaffinity(0, std::mem::size_of_val(&fast), &fast) != 0 {
                return None;
            }
            Some(Self(old))
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for CpuAffinity {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::sched_setaffinity(0, std::mem::size_of_val(&self.0), &self.0);
        }
    }
}

impl Drop for OpenMpPause {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        unsafe {
            // libgomp's omp_pause_soft releases active-wait workers after a
            // transcription, keeping the daemon quiet between dictations.
            let _ = omp_pause_resource_all(1);
        }
    }
}

fn parakeet_stream_options() -> transcribe_cpp::StreamOptions {
    transcribe_cpp::StreamOptions {
        family: Some(transcribe_cpp::StreamExtension::ParakeetBuffered(
            transcribe_cpp::ParakeetBufferedStreamOptions {
                left_ms: Some(5600),
                chunk_ms: Some(1040),
                right_ms: Some(1040),
            },
        )),
        ..Default::default()
    }
}

fn load_transcribe_model(path: PathBuf, config: &Config) -> Result<transcribe_cpp::Model> {
    let hybrid = optimization::profile(config) == "hybrid";
    if hybrid {
        std::env::set_var("TRANSCRIBE_CANARY_HYBRID", "1");
        std::env::set_var("GGML_VK_DISABLE_F16", "1");
    } else {
        std::env::remove_var("TRANSCRIBE_CANARY_HYBRID");
        std::env::remove_var("GGML_VK_DISABLE_F16");
    }
    Ok(transcribe_cpp::Model::load_with(
        path,
        &transcribe_cpp::ModelOptions {
            backend: if matches!(optimization::profile(config), "vulkan" | "hybrid") {
                transcribe_cpp::Backend::Vulkan
            } else {
                transcribe_cpp::Backend::Cpu
            },
            device: None,
        },
    )?)
}

pub enum StreamInput {
    Audio(Vec<f32>),
    Finish,
    Cancel,
}

pub fn model_path(name: &str) -> Result<PathBuf> {
    let local = expand(name);
    if local.is_dir() {
        return Ok(local);
    }
    let repo = match name {
        "large" => "Systran/faster-whisper-large-v3".into(),
        "turbo" | "large-v3-turbo" => "mobiuslabsgmbh/faster-whisper-large-v3-turbo".into(),
        "distil-large-v3.5" => "distil-whisper/distil-large-v3.5-ct2".into(),
        name if name.contains('/') => name.to_owned(),
        name if name.starts_with("distil-") => {
            format!("Systran/faster-distil-whisper-{}", &name[7..])
        }
        _ => format!("Systran/faster-whisper-{name}"),
    };
    let shared_cache = std::env::var_os("HF_HUB_CACHE")
        .or_else(|| std::env::var_os("HUGGINGFACE_HUB_CACHE"))
        .map(|path| hf_hub::Cache::new(path.into()))
        .unwrap_or_else(hf_hub::Cache::from_env);
    let cache = shared_cache.model(repo.clone());
    if let (Some(model), Some(_), Some(_)) = (
        cache.get("model.bin"),
        cache.get("tokenizer.json"),
        cache.get("config.json"),
    ) {
        return Ok(model.parent().unwrap().to_owned());
    }
    let api = hf_hub::api::sync::ApiBuilder::from_cache(shared_cache)
        .build()?
        .model(repo);
    api.get("config.json")?;
    api.get("tokenizer.json")?;
    let model = api.get("model.bin")?;
    Ok(model.parent().unwrap().to_owned())
}
pub fn read_audio(path: &Path, preprocess: bool) -> Result<Vec<f32>> {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(path);
    if preprocess {
        command.args([
            "-af",
            "highpass=f=80,lowpass=f=8000,loudnorm=I=-16:TP=-1.5:LRA=11",
        ]);
    }
    let bytes = process::run(
        command.args(["-ar", "16000", "-ac", "1", "-f", "f32le", "pipe:1"]),
        None,
        Duration::from_secs(30),
    )?;
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect())
}
pub enum Engine {
    Whisper {
        model: Whisper,
        tokenizer: Box<tokenizers::Tokenizer>,
        identity: (String, String),
    },
    Parakeet {
        session: transcribe_cpp::Session,
        identity: (String, String),
    },
    Canary {
        session: transcribe_cpp::Session,
        identity: (String, String),
    },
}
impl Engine {
    pub fn load(config: &Config) -> Result<Self> {
        let _pause_openmp_workers = OpenMpPause;
        optimization::check_profile(config)?;
        #[cfg(target_os = "linux")]
        let _performance_cores =
            CpuAffinity::performance_cores(optimization::profile(config) == "hybrid");
        let name = config.string("transcription", "model");
        let compute = config.string("transcription", "compute_type");
        if name == PARAKEET_MODEL || name.ends_with(".gguf") {
            let path = if name == PARAKEET_MODEL {
                let api = hf_hub::api::sync::Api::new()?
                    .model("handy-computer/parakeet-unified-en-0.6b-gguf".into());
                api.get("parakeet-unified-en-0.6b-Q8_0.gguf")?
            } else {
                let path = expand(name);
                if !path.is_file() {
                    bail!("Parakeet model does not exist: {}", path.display())
                }
                path
            };
            let model = load_transcribe_model(path, config)?;
            if !model.capabilities().supports_streaming {
                bail!("The selected Parakeet model does not support streaming")
            }
            let mut session = model.session_with(&transcribe_cpp::SessionOptions {
                n_threads: optimization::threads(config, 0) as i32,
                ..Default::default()
            })?;
            // transcribe.cpp prepares some kernels lazily. Exercise the actual
            // stream path while the daemon is loading in the background so the
            // user's first dictation does not pay that one-time cost.
            {
                let run = transcribe_cpp::RunOptions {
                    language: Some("en".into()),
                    ..Default::default()
                };
                let mut stream = session.stream(&run, &parakeet_stream_options())?;
                stream.feed(&vec![0.; 33_280])?;
                stream.finalize()?;
            }
            return Ok(Self::Parakeet {
                session,
                identity: optimization::identity(config),
            });
        }
        if name == CANARY_MODEL {
            let api = hf_hub::api::sync::Api::new()?
                .model("handy-computer/canary-180m-flash-gguf".into());
            let path = api.get("canary-180m-flash-Q8_0.gguf")?;
            let model = load_transcribe_model(path, config)?;
            let session = model.session_with(&transcribe_cpp::SessionOptions {
                n_threads: optimization::threads(config, 0) as i32,
                ..Default::default()
            })?;
            return Ok(Self::Canary {
                session,
                identity: optimization::identity(config),
            });
        }
        let compute_type = match compute {
            "int8" => ComputeType::INT8,
            "int8_float32" => ComputeType::INT8_FLOAT32,
            "float32" => ComputeType::FLOAT32,
            "float16" => ComputeType::FLOAT16,
            "int8_float16" => ComputeType::INT8_FLOAT16,
            "bfloat16" => ComputeType::BFLOAT16,
            "int16" => ComputeType::INT16,
            "default" => ComputeType::DEFAULT,
            "auto" => ComputeType::AUTO,
            _ => bail!("Unsupported compute type: {compute}"),
        };
        let path = model_path(name)?;
        let model = Whisper::new(
            &path,
            ct2rs::Config {
                compute_type,
                num_threads_per_replica: optimization::threads(config, 4),
                ..Default::default()
            },
        )?;
        let tokenizer = tokenizers::Tokenizer::from_file(path.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("Tokenizer: {e}"))?;
        Ok(Self::Whisper {
            model,
            tokenizer: Box::new(tokenizer),
            identity: optimization::identity(config),
        })
    }
    pub fn matches(&self, c: &Config) -> bool {
        let identity = match self {
            Self::Whisper { identity, .. }
            | Self::Parakeet { identity, .. }
            | Self::Canary { identity, .. } => identity,
        };
        *identity == optimization::identity(c)
    }
    pub fn set_identity(&mut self, requested: (String, String)) {
        match self {
            Self::Whisper { identity, .. }
            | Self::Parakeet { identity, .. }
            | Self::Canary { identity, .. } => *identity = requested,
        }
    }
    pub fn supports_streaming(&self) -> bool {
        matches!(self, Self::Parakeet { .. })
    }
    pub fn transcribe(&mut self, samples: &[f32], config: &Config) -> Result<String> {
        let _pause_openmp_workers = OpenMpPause;
        #[cfg(target_os = "linux")]
        let _performance_cores =
            CpuAffinity::performance_cores(optimization::profile(config) == "hybrid");
        if let Self::Parakeet { session, .. } = self {
            // Use the same buffered stream for history retries and benchmarks as live dictation.
            let mut stream = session.stream(
                &transcribe_cpp::RunOptions {
                    language: Some("en".into()),
                    ..Default::default()
                },
                &parakeet_stream_options(),
            )?;
            for chunk in samples.chunks(1600) {
                stream.feed(chunk)?;
            }
            stream.finalize()?;
            return Ok(stream.text().full);
        }
        if let Self::Canary { session, .. } = self {
            let language = config.string("transcription", "language");
            let options = transcribe_cpp::RunOptions {
                language: Some(if language.is_empty() { "en" } else { language }.into()),
                ..Default::default()
            };
            let mut text = Vec::new();
            // Canary is designed for <40 s inputs. Bound decoder output and
            // prefer quiet boundaries so long dictations do not cut words.
            let mut remaining = samples;
            while !remaining.is_empty() {
                let end = canary_chunk_end(remaining);
                let part = session.run(&remaining[..end], &options)?.text;
                if !part.trim().is_empty() {
                    text.push(part.trim().to_owned());
                }
                remaining = &remaining[end..];
            }
            return Ok(text.join(" "));
        }
        let Self::Whisper {
            model, tokenizer, ..
        } = self
        else {
            unreachable!()
        };
        let samples = if config.flag("transcription", "vad_filter") {
            speech_samples(samples)?
        } else {
            samples.to_vec()
        };
        let mut text = Vec::new();
        for chunk in samples.chunks(480000) {
            if chunk.len() < 1600 {
                continue;
            }
            let mut features = if matches!(optimization::profile(config), "fast" | "adaptive") {
                fast_log_mel(chunk, model.n_mels())
            } else {
                log_mel(chunk, model.n_mels())
            };
            let frames = optimization::context_frames(config, chunk.len());
            if frames < 3000 {
                features = features
                    .chunks_exact(3000)
                    .flat_map(|bin| bin[..frames].iter().copied())
                    .collect();
            }
            let view = StorageView::new(
                &[1, model.n_mels(), frames],
                &mut features,
                Default::default(),
            )?;
            let mut prompt = vec!["<|startoftranscript|>".to_string()];
            if model.is_multilingual() {
                let language = config.string("transcription", "language");
                let token = if language.is_empty() {
                    model
                        .detect_language(&view)?
                        .first()
                        .and_then(|d| d.first())
                        .context("Language detection returned no result")?
                        .language
                        .clone()
                } else {
                    format!("<|{language}|>")
                };
                if tokenizer.token_to_id(&token).is_none() {
                    bail!("Unsupported language: {language}")
                }
                prompt.extend([token, "<|transcribe|>".into()]);
            }
            prompt.push("<|notimestamps|>".into());
            let results = model.generate(
                &view,
                &[prompt],
                &WhisperOptions {
                    beam_size: config.number("transcription", "beam_size") as usize,
                    return_scores: true,
                    return_no_speech_prob: true,
                    ..Default::default()
                },
            )?;
            for result in results {
                if result.no_speech_prob > 0.6
                    && result.scores.first().copied().unwrap_or(-2.) < -1.
                {
                    continue;
                }
                if let Some(ids) = result.sequences_ids.first() {
                    let decoded = tokenizer
                        .decode(&ids.iter().map(|n| *n as u32).collect::<Vec<_>>(), true)
                        .map_err(|e| anyhow::anyhow!("Decode: {e}"))?;
                    if !decoded.trim().is_empty() {
                        text.push(decoded.trim().to_owned());
                    }
                }
            }
        }
        Ok(text.join(" "))
    }

    pub fn stream(
        &mut self,
        receive: &mpsc::Receiver<StreamInput>,
        mut preview: impl FnMut(String, String),
    ) -> Result<Option<String>> {
        let Self::Parakeet { session, .. } = self else {
            bail!("The selected model does not support streaming")
        };
        let run = transcribe_cpp::RunOptions {
            language: Some("en".into()),
            ..Default::default()
        };
        let options = parakeet_stream_options();
        let mut stream = session.stream(&run, &options)?;
        while let Ok(input) = receive.recv() {
            match input {
                StreamInput::Audio(samples) => {
                    let update = stream.feed(&samples)?;
                    if update.committed_changed || update.tentative_changed {
                        let text = stream.text();
                        preview(text.committed, text.tentative);
                    }
                }
                StreamInput::Finish => {
                    stream.finalize()?;
                    return Ok(Some(stream.text().full));
                }
                StreamInput::Cancel => {
                    stream.reset();
                    return Ok(None);
                }
            }
        }
        stream.reset();
        Ok(None)
    }
}
// At 16 kHz, look for the quietest 20 ms frame in the last five
// seconds of a 30 s window. Every sample belongs to exactly one chunk.
fn canary_chunk_end(samples: &[f32]) -> usize {
    const MAX: usize = 30 * 16000;
    const SEARCH: usize = 25 * 16000;
    const FRAME: usize = 320;
    if samples.len() <= MAX {
        return samples.len();
    }
    let (frame, _) = samples[SEARCH..MAX]
        .chunks_exact(FRAME)
        .enumerate()
        .map(|(i, frame)| (i, frame.iter().map(|v| v * v).sum::<f32>()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    SEARCH + frame * FRAME + FRAME / 2
}

/// Whisper's periodic Hann STFT, Slaney mel filters and global log compression.
pub fn log_mel(samples: &[f32], bins: usize) -> Vec<f32> {
    fn hz_to_mel(hz: f32) -> f32 {
        if hz < 1000. {
            hz / (200. / 3.)
        } else {
            15. + (hz / 1000.).ln() / (6.4_f32.ln() / 27.)
        }
    }
    fn mel_to_hz(m: f32) -> f32 {
        if m < 15. {
            m * (200. / 3.)
        } else {
            1000. * ((m - 15.) * (6.4_f32.ln() / 27.)).exp()
        }
    }
    let edges: Vec<_> = (0..bins + 2)
        .map(|i| mel_to_hz(hz_to_mel(8000.) * i as f32 / (bins + 1) as f32))
        .collect();
    let filters: Vec<Vec<(usize, f32)>> = (0..bins)
        .map(|b| {
            (0..201)
                .filter_map(|i| {
                    let hz = i as f32 * 40.;
                    let w = ((hz - edges[b]) / (edges[b + 1] - edges[b]))
                        .min((edges[b + 2] - hz) / (edges[b + 2] - edges[b + 1]))
                        .max(0.)
                        * 2.
                        / (edges[b + 2] - edges[b]);
                    (w > 0.).then_some((i, w))
                })
                .collect()
        })
        .collect();
    let fft = FftPlanner::new().plan_fft_forward(400);
    let mut buffer = vec![Complex32::default(); 400];
    let mut scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
    let mut output = vec![0.; bins * 3000];
    let mut maximum = f32::NEG_INFINITY;
    for frame in 0..3000 {
        for (i, v) in buffer.iter_mut().enumerate() {
            let index = (frame as isize * 160 + i as isize - 200).unsigned_abs();
            let sample = samples.get(index).copied().unwrap_or(0.);
            *v = Complex32::new(
                sample * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / 400.).cos()),
                0.,
            );
        }
        fft.process_with_scratch(&mut buffer, &mut scratch);
        for (b, filter) in filters.iter().enumerate() {
            let value = filter
                .iter()
                .map(|(i, w)| buffer[*i].norm_sqr() * w)
                .sum::<f32>()
                .max(1e-10)
                .log10();
            output[b * 3000 + frame] = value;
            maximum = maximum.max(value);
        }
    }
    for v in &mut output {
        *v = (v.max(maximum - 8.) + 4.) / 4.;
    }
    output
}
pub fn speech_samples(samples: &[f32]) -> Result<Vec<f32>> {
    let mut vad = voice_activity_detector::VoiceActivityDetector::builder()
        .sample_rate(16000_i64)
        .chunk_size(512_usize)
        .build()?;
    let chunks: Vec<_> = samples.chunks(512).collect();
    let probabilities: Vec<_> = chunks
        .iter()
        .map(|s| vad.predict(s.iter().copied()))
        .collect();
    // Keep 400 ms padding around speech, merge gaps shorter than two seconds.
    let mut spans = Vec::new();
    let mut start = None;
    let mut last = 0;
    for (i, p) in probabilities.iter().enumerate() {
        if *p >= 0.5 {
            start.get_or_insert(i.saturating_sub(13));
            last = i;
        }
        if start.is_some() && i > last + 63 {
            spans.push((start.take().unwrap(), (last + 14).min(chunks.len())));
        }
    }
    if let Some(start) = start {
        spans.push((start, (last + 14).min(chunks.len())));
    }
    Ok(spans
        .into_iter()
        .flat_map(|(a, b)| chunks[a..b].iter().flat_map(|s| s.iter().copied()))
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canary_chunks_preserve_audio_and_choose_quiet_boundaries() {
        let mut samples = vec![0.5; 95 * 16000 + 123];
        samples[28 * 16000..28 * 16000 + 320].fill(0.);
        assert_eq!(canary_chunk_end(&samples), 28 * 16000 + 160);
        let mut consumed = 0;
        while consumed < samples.len() {
            let end = canary_chunk_end(&samples[consumed..]);
            assert!(end > 0 && end <= 30 * 16000);
            consumed += end;
        }
        assert_eq!(consumed, samples.len());
        assert_eq!(canary_chunk_end(&[]), 0);
        assert_eq!(canary_chunk_end(&samples[..30 * 16000]), 30 * 16000);
    }
    #[test]
    fn mel_features_match_faster_whisper_reference() {
        // NumPy/faster-whisper reference for a 440 Hz + 1700 Hz signal, zero-padded to 30 s.
        let samples: Vec<f32> = (0..16000)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / 16000.;
                (0.3 * (440. * t).sin() + 0.1 * (1700. * t).sin()) as f32
            })
            .collect();
        for (bins, reference) in [
            (
                80,
                [
                    0.8894523, 1.2393988, 1.1825976, -0.6727201, 1.0416309, -0.6727201, -0.6727201,
                ],
            ),
            (
                128,
                [
                    0.81369305,
                    0.33110625,
                    -0.6255697,
                    1.1764674,
                    -0.13629544,
                    -0.6255697,
                    -0.6255697,
                ],
            ),
        ] {
            let features = log_mel(&samples, bins);
            let coordinates = [
                (0, 0),
                (10, 1),
                (12, 25),
                (20, 50),
                (40, 99),
                (bins - 1, 10),
                (0, 200),
            ];
            for ((bin, frame), expected) in coordinates.into_iter().zip(reference) {
                assert!(
                    (features[bin * 3000 + frame] - expected).abs() < 0.0001,
                    "{bins} mel bins at {bin}/{frame}: {} != {expected}",
                    features[bin * 3000 + frame]
                );
            }
        }
    }
    #[test]
    fn silent_features_are_finite_and_have_expected_shape() {
        let m = log_mel(&vec![0.; 16000], 80);
        assert_eq!(m.len(), 240000);
        assert!(m.iter().all(|v| (*v + 1.5).abs() < 1e-6));
    }
}

/// Equivalent full-context features, skipping FFT work on zero padding.
pub fn fast_log_mel(samples: &[f32], bins: usize) -> Vec<f32> {
    fn hz_to_mel(hz: f32) -> f32 {
        if hz < 1000. {
            hz / (200. / 3.)
        } else {
            15. + (hz / 1000.).ln() / (6.4_f32.ln() / 27.)
        }
    }
    fn mel_to_hz(m: f32) -> f32 {
        if m < 15. {
            m * (200. / 3.)
        } else {
            1000. * ((m - 15.) * (6.4_f32.ln() / 27.)).exp()
        }
    }
    let edges: Vec<_> = (0..bins + 2)
        .map(|i| mel_to_hz(hz_to_mel(8000.) * i as f32 / (bins + 1) as f32))
        .collect();
    let filters: Vec<Vec<(usize, f32)>> = (0..bins)
        .map(|b| {
            (0..201)
                .filter_map(|i| {
                    let hz = i as f32 * 40.;
                    let w = ((hz - edges[b]) / (edges[b + 1] - edges[b]))
                        .min((edges[b + 2] - hz) / (edges[b + 2] - edges[b + 1]))
                        .max(0.)
                        * 2.
                        / (edges[b + 2] - edges[b]);
                    (w > 0.).then_some((i, w))
                })
                .collect()
        })
        .collect();
    let fft = FftPlanner::new().plan_fft_forward(400);
    let mut buffer = vec![Complex32::default(); 400];
    let mut scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
    let mut output = vec![-10.; bins * 3000];
    let mut maximum = -10_f32;
    let window: Vec<f32> = (0..400)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / 400.).cos())
        .collect();
    let frames = samples.len().saturating_add(200).div_ceil(160).min(3000);
    for frame in 0..frames {
        for (i, v) in buffer.iter_mut().enumerate() {
            let index = (frame as isize * 160 + i as isize - 200).unsigned_abs();
            let sample = samples.get(index).copied().unwrap_or(0.);
            *v = Complex32::new(sample * window[i], 0.);
        }
        fft.process_with_scratch(&mut buffer, &mut scratch);
        for (b, filter) in filters.iter().enumerate() {
            let value = filter
                .iter()
                .map(|(i, w)| buffer[*i].norm_sqr() * w)
                .sum::<f32>()
                .max(1e-10)
                .log10();
            output[b * 3000 + frame] = value;
            maximum = maximum.max(value);
        }
    }
    for v in &mut output {
        *v = (v.max(maximum - 8.) + 4.) / 4.;
    }
    output
}

#[cfg(test)]
mod optimization_tests {
    use super::*;
    #[test]
    fn fast_features_match_reference_at_padding_boundaries() {
        for n in [0, 1, 159, 160, 199, 200, 319, 80_000, 479_999, 480_000] {
            let samples: Vec<f32> = (0..n)
                .map(|i| (i as f32 * 0.1321).sin() * 0.3 + (i % 17) as f32 * 0.0001)
                .collect();
            for bins in [80, 128] {
                let a = log_mel(&samples, bins);
                let b = fast_log_mel(&samples, bins);
                assert!(
                    a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits()),
                    "length {n}, bins {bins}"
                );
            }
        }
    }
}
