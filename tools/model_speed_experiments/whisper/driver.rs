use rustfft::{FftPlanner,num_complex::Complex32};
use anyhow::{bail,Context,Result};
use ct2rs::{sys::{Whisper,WhisperOptions,StorageView}, ComputeType};
use serde_json::json;
use std::{path::Path, time::Instant};
use voice_dictation::{config::Config, engine::Engine};
extern "C" { fn omp_pause_resource_all(kind: i32) -> i32; }
fn usage() -> (f64,i64) {
    let mut r=std::mem::MaybeUninit::<libc::rusage>::zeroed();
    unsafe { assert_eq!(libc::getrusage(libc::RUSAGE_SELF,r.as_mut_ptr()),0);let r=r.assume_init();
    (r.ru_utime.tv_sec as f64+r.ru_stime.tv_sec as f64+(r.ru_utime.tv_usec+r.ru_stime.tv_usec) as f64/1e6,r.ru_maxrss) }
}
fn main() -> Result<()> {
    if std::env::var_os("BENCH_CHECK_MEL").is_some() {
        for n in [0,1,159,160,199,200,319,80000,479999,480000] {
            let samples:Vec<f32>=(0..n).map(|i| ((i as f32*0.1321).sin()*0.3)+(i%17) as f32*0.0001).collect();
            for bins in [80,128] {
                let a=voice_dictation::engine::log_mel(&samples,bins);let b=fast_log_mel(&samples,bins);
                anyhow::ensure!(a.iter().zip(&b).all(|(a,b)|a.to_bits()==b.to_bits()),"mel mismatch n={n}, bins={bins}");
            }
        }
        println!("20 mel comparisons are bit-identical"); return Ok(());
    }
    let args:Vec<_>=std::env::args().collect(); anyhow::ensure!(args.len()>=4,"model rounds clip.f32...");
    let model_dir=Path::new(&args[1]);
    let threads=std::env::var("BENCH_THREADS").ok().and_then(|v|v.parse().ok()).unwrap_or(4);
    let temp=tempfile::tempdir()?;let mut config=Config::at(temp.path())?;
    config.data["transcription"]["model"]=json!(args[1]);
    config.data["transcription"]["compute_type"]=json!("int8");
    config.data["transcription"]["beam_size"]=json!(std::env::var("BENCH_BEAM").ok().and_then(|v|v.parse::<usize>().ok()).unwrap_or(5));
    config.data["transcription"]["vad_filter"]=json!(std::env::var("BENCH_VAD").as_deref()!=Ok("0"));
    let start=Instant::now();
    let model=Whisper::new(model_dir,ct2rs::Config {compute_type:ComputeType::INT8,num_threads_per_replica:threads,..Default::default()})?;
    let tokenizer=tokenizers::Tokenizer::from_file(model_dir.join("tokenizer.json")).map_err(|e|anyhow::anyhow!("{e}"))?;
    let mut engine=Engine::Whisper {model,tokenizer:Box::new(tokenizer),identity:(args[1].clone(),"int8".into())};
    println!("{}",json!({"event":"loaded","seconds":start.elapsed().as_secs_f64(),"peak_rss_kib":usage().1,"threads":threads}));
    for round in 0..args[2].parse::<usize>()? {
        for path in &args[3..] {
            let bytes=std::fs::read(path)?;let audio:Vec<f32>=bytes.chunks_exact(4).map(|b|f32::from_le_bytes(b.try_into().unwrap())).collect();
            let start=Instant::now();let cpu=usage().0;
            let text=if std::env::var_os("BENCH_CUSTOM").is_some() {candidate(&mut engine,&audio,&config)?} else {engine.transcribe(&audio,&config)?};
            if std::env::var_os("BENCH_PAUSE_OMP").is_some() {anyhow::ensure!(unsafe{omp_pause_resource_all(1)}==0,"pause failed");}
            println!("{}",json!({"event":"transcribed","round":round,"audio":path,"seconds":start.elapsed().as_secs_f64(),"cpu_seconds":usage().0-cpu,"peak_rss_kib":usage().1,"text":text}));
        }
    }
    let cpu=usage().0;std::thread::sleep(std::time::Duration::from_millis(500));
    println!("{}",json!({"event":"idle","cpu_seconds_over_half_second":usage().0-cpu}));
    Ok(())
}

fn candidate(engine: &mut Engine, samples: &[f32], config: &Config) -> Result<String> {
    let Engine::Whisper {model,tokenizer,..}=engine else {bail!("whisper required")};
        let samples = if config.flag("transcription", "vad_filter") {
            voice_dictation::engine::speech_samples(samples)?
        } else {
            samples.to_vec()
        };
        let mut text = Vec::new();
        for chunk in samples.chunks(480000) {
            if chunk.len() < 1600 {
                continue;
            }
            let mel_start=Instant::now();
            let mut features = if std::env::var_os("BENCH_FAST_MEL").is_some() {fast_log_mel(chunk,model.n_mels())} else {voice_dictation::engine::log_mel(chunk, model.n_mels())};
            let frames = if std::env::var_os("BENCH_SHORT_ONLY").is_some() && chunk.len() > 5*16000 {
                3000
            } else if let Ok(pad) = std::env::var("BENCH_PAD_SECONDS") {
                ((chunk.len().div_ceil(160) + pad.parse::<usize>()?*100).div_ceil(2)*2).clamp(std::env::var("BENCH_MIN_SECONDS").ok().and_then(|v|v.parse::<usize>().ok()).unwrap_or(1)*100,3000)
            } else { 3000 };
            if frames < 3000 { features=features.chunks_exact(3000).flat_map(|bin|bin[..frames].iter().copied()).collect(); }
            eprintln!("MEL_MS {} FRAMES {}",mel_start.elapsed().as_secs_f64()*1000.,frames);
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

fn fast_log_mel(samples: &[f32], bins: usize) -> Vec<f32> {
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
    let window: Vec<f32>=(0..400).map(|i|0.5-0.5*(std::f32::consts::TAU*i as f32/400.).cos()).collect();
    let frames = samples.len().saturating_add(200).div_ceil(160).min(3000);
    for frame in 0..frames {
        for (i, v) in buffer.iter_mut().enumerate() {
            let index = (frame as isize * 160 + i as isize - 200).unsigned_abs();
            let sample = samples.get(index).copied().unwrap_or(0.);
            *v = Complex32::new(
                sample * window[i],
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