use crate::{
    config::{expand, Config},
    process,
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
    time::Duration,
};

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
pub struct Engine {
    model: Whisper,
    tokenizer: tokenizers::Tokenizer,
    pub identity: (String, String),
}
impl Engine {
    pub fn load(config: &Config) -> Result<Self> {
        let name = config.string("transcription", "model");
        let compute = config.string("transcription", "compute_type");
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
                num_threads_per_replica: 4,
                ..Default::default()
            },
        )?;
        let tokenizer = tokenizers::Tokenizer::from_file(path.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("Tokenizer: {e}"))?;
        Ok(Self {
            model,
            tokenizer,
            identity: (name.into(), compute.into()),
        })
    }
    pub fn matches(&self, c: &Config) -> bool {
        self.identity.0 == c.string("transcription", "model")
            && self.identity.1 == c.string("transcription", "compute_type")
    }
    pub fn transcribe(&self, samples: &[f32], config: &Config) -> Result<String> {
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
            let mut features = log_mel(chunk, self.model.n_mels());
            let view = StorageView::new(
                &[1, self.model.n_mels(), 3000],
                &mut features,
                Default::default(),
            )?;
            let mut prompt = vec!["<|startoftranscript|>".to_string()];
            if self.model.is_multilingual() {
                let language = config.string("transcription", "language");
                let token = if language.is_empty() {
                    self.model
                        .detect_language(&view)?
                        .first()
                        .and_then(|d| d.first())
                        .context("Language detection returned no result")?
                        .language
                        .clone()
                } else {
                    format!("<|{language}|>")
                };
                if self.tokenizer.token_to_id(&token).is_none() {
                    bail!("Unsupported language: {language}")
                }
                prompt.extend([token, "<|transcribe|>".into()]);
            }
            prompt.push("<|notimestamps|>".into());
            let results = self.model.generate(
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
                    let decoded = self
                        .tokenizer
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
