//! User-controlled performance profiles and isolated, cancellable local measurements.
use crate::{config::Config, engine::Engine};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn profile(c: &Config) -> &str {
    c.data["performance"]["profile"]
        .as_str()
        .unwrap_or("standard")
}
fn valid_setting(setting: &Value) -> bool {
    setting.is_object()
        && ["standard", "fast", "adaptive", "vulkan"]
            .contains(&setting["profile"].as_str().unwrap_or(""))
        && setting["threads"].as_u64().is_some_and(|n| n <= 256)
}
fn compatible(model: &str, setting: &Value) -> bool {
    let gguf = model == crate::engine::PARAKEET_MODEL
        || model == crate::engine::CANARY_MODEL
        || model.ends_with(".gguf");
    match setting["profile"].as_str().unwrap_or("") {
        "vulkan" => gguf,
        "fast" => !gguf,
        "adaptive" => model == "base.en",
        "standard" => true,
        _ => false,
    }
}
pub fn validate(d: &Value) -> Result<()> {
    if d["performance"].is_null() {
        return Ok(());
    }
    if !d["performance"].is_object() || !valid_setting(&d["performance"]) {
        bail!("Performance requires a known profile and a thread count between 0 and 256");
    }
    let saved = &d["performance"]["by_model"];
    if !saved.is_null() && !saved.is_object() {
        bail!("performance.by_model must be a mapping");
    }
    if let Some(saved) = saved.as_object() {
        for (model, setting) in saved {
            if model.is_empty() || !valid_setting(setting) || !compatible(model, setting) {
                bail!("Invalid saved performance setting for model {model}");
            }
        }
    }
    Ok(())
}
pub fn current_setting(c: &Config) -> Value {
    json!({"profile":profile(c),"threads":c.number("performance", "threads")})
}
pub fn remember_current(c: &mut Config) {
    let model = c.string("transcription", "model").to_owned();
    let setting = current_setting(c);
    c.data["performance"]["by_model"][model] = setting;
}
pub fn remember(c: &mut Config, model: &str, setting: Value) {
    c.data["performance"]["by_model"][model] = setting;
}
pub fn activate_saved(c: &mut Config) {
    let model = c.string("transcription", "model").to_owned();
    let setting = c.data["performance"]["by_model"]
        .get(&model)
        .cloned()
        .unwrap_or_else(|| json!({"profile":"standard","threads":0}));
    c.data["performance"]["profile"] = setting["profile"].clone();
    c.data["performance"]["threads"] = setting["threads"].clone();
    if check_profile(c).is_err() {
        c.data["performance"]["profile"] = json!("standard");
        c.data["performance"]["threads"] = json!(0);
    }
}
pub fn is_gguf(c: &Config) -> bool {
    let name = c.string("transcription", "model");
    name == crate::engine::PARAKEET_MODEL
        || name == crate::engine::CANARY_MODEL
        || name.ends_with(".gguf")
}
pub fn check_profile(c: &Config) -> Result<()> {
    match profile(c) {
        "vulkan" if !cfg!(feature = "vulkan") => {
            bail!("This CPU-only build does not include Vulkan")
        }
        "vulkan" if !is_gguf(c) => {
            bail!("Vulkan is available for GGUF models, not this Whisper engine")
        }
        "fast" | "adaptive" if is_gguf(c) => bail!("Choose Standard CPU or Vulkan for this model"),
        "adaptive" if c.string("transcription", "model") != "base.en" => {
            bail!("Experimental short context is currently limited to Whisper base.en")
        }
        _ => Ok(()),
    }
}
pub fn threads(c: &Config, default: usize) -> usize {
    match c.number("performance", "threads") as usize {
        0 => default,
        n => n,
    }
}
pub fn identity(c: &Config) -> (String, String) {
    (
        c.string("transcription", "model").into(),
        format!(
            "{} / {} / {} threads",
            c.string("transcription", "compute_type"),
            profile(c),
            c.number("performance", "threads")
        ),
    )
}
pub fn context_frames(c: &Config, samples: usize) -> usize {
    if profile(c) == "adaptive"
        && c.string("transcription", "model") == "base.en"
        && samples <= 80_000
    {
        1000
    } else {
        3000
    }
}
pub fn durations(value: &Value) -> Result<Vec<u64>> {
    let ds = value.as_array().context("Choose benchmark durations")?;
    if ds.is_empty() || ds.len() > 5 {
        bail!("Choose between one and five durations");
    }
    let mut result = Vec::new();
    for d in ds {
        let n = d.as_u64().context("Duration must be an integer")?;
        if ![2, 5, 15, 30, 60].contains(&n) || result.contains(&n) {
            bail!("Durations must be distinct: 2, 5, 15, 30 or 60 seconds");
        }
        result.push(n);
    }
    Ok(result)
}
fn normalized(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}
fn host() -> Value {
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|s| {
            s.strip_prefix("model name")
                .and_then(|s| s.split_once(':'))
                .map(|(_, s)| s.trim().to_owned())
        })
        .unwrap_or_default();
    json!({"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"cpu":cpu,
        "logical_cpus":thread::available_parallelism().map(|n|n.get()).unwrap_or(1),
        "version":env!("CARGO_PKG_VERSION"),"kernel":fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim(),"native_engine":"transcribe.cpp 0.2.3 / CTranslate2 via ct2rs 0.10.1","protocol":1,"fixture":"synthetic-english-v1","vulkan_build":cfg!(feature="vulkan")})
}
fn ram(pid: u32) -> (Option<f64>, Option<f64>) {
    let rss = fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("VmHWM:")
                    .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
                    .map(|v| v / 1024.)
            })
        });
    let mut clients = HashMap::new();
    let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fdinfo")) else {
        return (rss, None);
    };
    for entry in fds.flatten() {
        let Ok(s) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let id = s
            .lines()
            .find_map(|l| l.strip_prefix("drm-client-id:"))
            .map(str::trim);
        let resident = s
            .lines()
            .filter_map(|l| {
                let (key, value) = l.split_once(':')?;
                if !key.starts_with("drm-resident-") {
                    return None;
                }
                let mut fields = value.split_whitespace();
                let value = fields.next()?.parse::<f64>().ok()?;
                Some(
                    value
                        * match fields.next() {
                            Some("KiB") => 1024.,
                            Some("MiB") => 1048576.,
                            _ => 1.,
                        },
                )
            })
            .collect::<Vec<f64>>();
        if let Some(id) = id {
            if !resident.is_empty() {
                clients.insert(id.to_owned(), resident.iter().sum::<f64>());
            }
        }
    }
    let gpu = (!clients.is_empty()).then(|| clients.values().sum::<f64>() / 1048576.);
    (rss, gpu)
}
fn peak(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Runs only in a disposable process, before loading the user's configuration.
pub fn child(request: &Path, output: &Path) -> Result<()> {
    let v: Value = serde_json::from_slice(&fs::read(request)?)?;
    // Keep child scratch under the runner's directory so cancellation can remove it
    // even when this process is killed before Rust destructors run.
    let parent = request
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let root = tempfile::tempdir_in(parent)?;
    let config = Config::at(root.path())?.changed(&v["config"])?;
    check_profile(&config)?;
    let duration = durations(&json!([v["seconds"]]))?[0];
    let audio = root.path().join("speech.flac");
    fs::write(&audio, include_bytes!("../assets/benchmarks/speech.flac"))?;
    let mut samples = crate::engine::read_audio(&audio, false)?;
    samples.truncate(duration as usize * 16000);
    anyhow::ensure!(
        samples.len() == duration as usize * 16000,
        "Benchmark fixture is incomplete"
    );
    let start = Instant::now();
    let mut engine = Engine::load(&config)?;
    let load = start.elapsed().as_secs_f64();
    let mut times = Vec::new();
    let mut texts = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let text = engine.transcribe(&samples, &config)?;
        times.push(start.elapsed().as_secs_f64());
        texts.push(text);
    }
    let memory = ram(std::process::id());
    let result = json!({"schema_version":1,"performance":config.data["performance"],"transcription":config.data["transcription"],"gpu_memory_measurement":"resident_after_calls","seconds":duration,"load_seconds":load,"warmup_seconds":times[0],
        "warm_seconds":(times[1]+times[2])/2.,"samples_seconds":&times[1..],
        "text":texts[2],"stable_words":normalized(&texts[0])==normalized(&texts[1]) && normalized(&texts[1])==normalized(&texts[2]),
        "process_peak_mib":memory.0,"gpu_resident_mib":memory.1,
        "devices": if profile(&config)=="vulkan" { crate::optimization::device_info() } else {json!([])} });
    fs::write(output, serde_json::to_vec(&result)?)?;
    Ok(())
}
pub fn device_info() -> Value {
    json!(transcribe_cpp::devices()
        .into_iter()
        .map(|d| json!({"name":d.name,"description":d.description,"kind":d.kind,"id":d.device_id}))
        .collect::<Vec<_>>())
}

fn measure(executable: &Path, config: &Config, seconds: u64, cancel: &AtomicBool) -> Result<Value> {
    let root = tempfile::tempdir()?;
    let request = root.path().join("request.json");
    let output = root.path().join("result.json");
    let errors = root.path().join("stderr.log");
    fs::write(
        &request,
        serde_json::to_vec(&json!({"config":config.data,"seconds":seconds}))?,
    )?;
    let mut command = Command::new(executable);
    crate::process::kill_with_parent(&mut command);
    let mut child = command
        .arg("--benchmark-case")
        .arg(&request)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&errors)?)
        .spawn()
        .context("Start benchmark process")?;
    let start = Instant::now();
    let mut memory = (None, None);
    let status = loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(if cancel.load(Ordering::Relaxed) {
                "Benchmark cancelled"
            } else {
                "Benchmark timed out after five minutes for one case"
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
        let current = ram(child.id());
        memory.0 = peak(memory.0, current.0);
        memory.1 = peak(memory.1, current.1);
        thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        let log = fs::read_to_string(errors).unwrap_or_default();
        bail!(
            "Benchmark process failed ({status}): {}",
            log.chars()
                .rev()
                .take(1500)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
    }
    let mut result: Value = serde_json::from_slice(&fs::read(output)?)?;
    result["gpu_memory_measurement"] = json!("sampled_peak_100ms");
    result["process_peak_mib"] = json!(peak(memory.0, result["process_peak_mib"].as_f64()));
    result["gpu_resident_mib"] = json!(peak(memory.1, result["gpu_resident_mib"].as_f64()));
    Ok(result)
}
pub fn run(
    config: &Config,
    ds: &[u64],
    cancel: &AtomicBool,
    mut progress: impl FnMut(Value),
) -> Result<Value> {
    check_profile(config)?;
    let executable = std::env::current_exe()?;
    let baseline = config.changed(&json!({"performance":{"profile":"standard","threads":0}}))?;
    let mut rows = Vec::new();
    for &seconds in ds {
        let mut pair = Vec::new();
        for (name, c) in [("reference", &baseline), ("candidate", config)] {
            progress(
                json!({"type":"benchmark","running":true,"message":format!("{seconds}s clip · {name} · warmup + two runs")}),
            );
            pair.push(measure(&executable, c, seconds, cancel)?);
        }
        rows.push(json!({"seconds":seconds,"reference":pair[0],"candidate":pair[1],
            "speedup":pair[0]["warm_seconds"].as_f64().unwrap()/pair[1]["warm_seconds"].as_f64().unwrap().max(0.000001),
            "same_words":normalized(pair[0]["text"].as_str().unwrap())==normalized(pair[1]["text"].as_str().unwrap())}));
    }
    let report = json!({"schema_version":1,"created":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),"host":host(),
        "transcription":config.data["transcription"],"performance":config.data["performance"],"rows":rows,
        "memory_note":"Process peak RSS and sampled resident GPU buffers in MiB, measured separately. Integrated GPUs use system RAM; peaks may overlap. Unavailable counters are null.",
        "method":"Synthetic English speech, one warmup and two warm measurements per fresh process. Same words compares against the standard CPU path, not a reference transcript. Parakeet uses streaming compute without real-time audio delays."});
    fs::create_dir_all(config.data_dir.join("benchmarks"))?;
    let path = config.data_dir.join("benchmarks").join(format!(
        "{}-{}.json",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id()
    ));
    fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}
pub fn reports(c: &Config) -> Value {
    let mut paths: Vec<_> = fs::read_dir(c.data_dir.join("benchmarks"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    json!(paths
        .into_iter()
        .rev()
        .take(8)
        .filter_map(|p| fs::read(p)
            .ok()
            .and_then(|s| serde_json::from_slice::<Value>(&s).ok()))
        .collect::<Vec<_>>())
}

/// Terminal interface; benchmarking and applying a profile are separate actions.
pub fn cli(args: &[String], config: &Config) -> Result<bool> {
    let has = |name: &str| args.iter().any(|s| s == name);
    let actions = [
        "--list-optimizations",
        "--performance-status",
        "--benchmark",
        "--benchmark-results",
        "--apply-optimization",
    ];
    let count = actions.iter().filter(|a| has(a)).count();
    if count == 0 {
        return Ok(false);
    }
    anyhow::ensure!(count == 1, "Choose one performance command at a time");
    let mut index = 1;
    let mut seen = Vec::new();
    while index < args.len() {
        let flag = args[index].as_str();
        anyhow::ensure!(!seen.contains(&flag), "Duplicate option: {flag}");
        seen.push(flag);
        if [
            "--apply-optimization",
            "--profile",
            "--threads",
            "--model",
            "--seconds",
        ]
        .contains(&flag)
        {
            anyhow::ensure!(
                args.get(index + 1).is_some_and(|v| !v.starts_with("--")),
                "{flag} requires a value"
            );
            index += 2;
        } else if actions.contains(&flag) {
            index += 1;
        } else {
            bail!("Unknown performance option: {flag}");
        }
    }
    let value = |name: &str| -> Result<Option<&str>> {
        args.iter()
            .position(|s| s == name)
            .map(|i| {
                args.get(i + 1)
                    .filter(|s| !s.starts_with("--"))
                    .map(String::as_str)
                    .with_context(|| format!("{name} requires a value"))
            })
            .transpose()
    };
    if has("--list-optimizations") {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schema_version":1,"host":host(),"threads":{"default":0,"maximum":256},"profiles":[
                    {"id":"standard","models":"all","experimental":false,"compiled":true},
                    {"id":"fast","models":"whisper","experimental":false,"compiled":true,"numerically_exact":true},
                    {"id":"adaptive","models":["base.en"],"experimental":true,"compiled":true,"short_context_max_seconds":5},
                    {"id":"vulkan","models":"gguf","experimental":false,"compiled":cfg!(feature="vulkan"),"runtime_check":"benchmark"}
                ]})
            )?
        );
        return Ok(true);
    }
    if has("--performance-status") {
        crate::ipc::ensure_daemon()?;
        use std::io::{BufRead, Read};
        let stream = std::os::unix::net::UnixStream::connect(crate::ipc::socket_path())?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut bytes = Vec::new();
        std::io::BufReader::new(stream)
            .take(1_048_577)
            .read_until(b'\n', &mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= 1_048_576,
            "Service response exceeded one MiB"
        );
        let state: Value = serde_json::from_slice(&bytes)?;
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(true);
    }
    if has("--benchmark-results") {
        println!("{}", serde_json::to_string_pretty(&reports(config))?);
        return Ok(true);
    }
    let benchmarking = has("--benchmark");
    let applying = has("--apply-optimization");
    if !benchmarking && !applying {
        return Ok(false);
    }
    anyhow::ensure!(
        !(benchmarking && applying),
        "Benchmark and apply are separate commands"
    );
    let mut changes = json!({});
    if let Some(p) = value(if applying {
        "--apply-optimization"
    } else {
        "--profile"
    })? {
        changes["performance"]["profile"] = json!(p);
    }
    if let Some(n) = value("--threads")? {
        changes["performance"]["threads"] =
            json!(n.parse::<u64>().context("Threads must be an integer")?);
    }
    if let Some(model) = value("--model")? {
        changes["transcription"]["model"] = json!(model);
    }
    let candidate = config.changed(&changes)?;
    check_profile(&candidate)?;
    let request = if benchmarking {
        let ds: Vec<u64> = value("--seconds")?
            .unwrap_or("5")
            .split(',')
            .map(|s| {
                s.parse::<u64>()
                    .context("Use comma-separated seconds, e.g. 2,5,15,30,60")
            })
            .collect::<Result<_>>()?;
        durations(&json!(ds))?;
        json!({"cmd":"benchmark","transcription":candidate.data["transcription"],"performance":candidate.data["performance"],"durations":ds})
    } else {
        json!({"cmd":"save_config","config":changes,"remember_performance":true})
    };
    crate::ipc::ensure_daemon()?;
    use std::io::{Read, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(crate::ipc::socket_path())?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let signal = signal_hook::flag::register(signal_hook::consts::SIGINT, cancelled.clone())?;
    let result = (|| -> Result<()> {
        if !benchmarking {
            writeln!(stream, "{request}")?;
        }
        let mut sent_request = !benchmarking;
        let waiting = Instant::now();
        let mut pending = Vec::new();
        let mut block = [0; 8192];
        let mut sent_cancel = false;
        loop {
            if !sent_request && waiting.elapsed() > Duration::from_secs(300) {
                bail!("Timed out waiting for the selected model");
            }
            if cancelled.load(Ordering::Relaxed) && !sent_cancel {
                if !sent_request {
                    bail!("Cancelled before benchmarking started");
                }
                if !benchmarking {
                    bail!("Interrupted; check the saved configuration before retrying");
                }
                writeln!(stream, "{}", json!({"cmd":"cancel_benchmark"}))?;
                sent_cancel = true;
                eprintln!("Cancelling benchmark…");
            }
            match stream.read(&mut block) {
                Ok(0) => bail!("Speech service disconnected"),
                Ok(n) => pending.extend_from_slice(&block[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Err(e) => return Err(e.into()),
            }
            anyhow::ensure!(
                pending.len() <= 1_048_576,
                "Service response exceeded one MiB"
            );
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=end).collect();
                let v: Value = serde_json::from_slice(&line)?;
                match v["type"].as_str().unwrap_or("") {
                    "state" if benchmarking && !sent_request => {
                        anyhow::ensure!(
                            v["benchmarking"] != true
                                && v["recording"] != true
                                && v["processing"] != true,
                            "Finish pending dictation or the current benchmark first"
                        );
                        if v["model_ready"] == true {
                            writeln!(stream, "{request}")?;
                            sent_request = true;
                        } else if v["last_error"].as_str().is_some_and(|s| !s.is_empty()) {
                            bail!("Selected model is not ready: {}", v["last_error"]);
                        }
                    }
                    "error" => bail!("{}", v["message"].as_str().unwrap_or("Service error")),
                    "config_reloaded" if applying => {
                        println!(
                            "{}",
                            json!({"saved":true,"model":candidate.string("transcription","model"),"performance":candidate.data["performance"],"verify_with":"--performance-status"})
                        );
                        return Ok(());
                    }
                    "benchmark" if benchmarking => {
                        eprintln!("{}", v["message"].as_str().unwrap_or(""));
                        if v["running"] == false {
                            anyhow::ensure!(
                                v["report"].is_object(),
                                "{}",
                                v["message"].as_str().unwrap_or("Benchmark failed")
                            );
                            println!("{}", serde_json::to_string_pretty(&v["report"])?);
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }
        }
    })();
    signal_hook::low_level::unregister(signal);
    result?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_reload_and_short_context_keeps_one_model() {
        let root = tempfile::tempdir().unwrap();
        let c=Config::at(root.path()).unwrap().changed(&json!({"transcription":{"model":"base.en"},"performance":{"profile":"adaptive","threads":2}})).unwrap();
        assert!(check_profile(&c).is_ok());
        assert_eq!(context_frames(&c, 80000), 1000);
        assert_eq!(context_frames(&c, 80001), 3000);
        let standard = c
            .changed(&json!({"performance":{"profile":"standard"}}))
            .unwrap();
        assert_ne!(identity(&c), identity(&standard));
        assert!(check_profile(
            &c.changed(&json!({"transcription":{"model":"large-v3"}}))
                .unwrap()
        )
        .is_err());
        assert!(c.changed(&json!({"performance":{"threads":257}})).is_err());
        assert!(durations(&json!([5, 5])).is_err());
        assert!(durations(&json!([2, 5, 15, 30, 60])).is_ok());
    }
    #[test]
    fn model_settings_are_remembered_and_restored_independently() {
        let root = tempfile::tempdir().unwrap();
        let mut c = Config::at(root.path())
            .unwrap()
            .changed(&json!({
                "transcription":{"model":"base.en"},
                "performance":{"profile":"adaptive","threads":8}
            }))
            .unwrap();
        remember_current(&mut c);
        c.data["transcription"]["model"] = json!(crate::engine::CANARY_MODEL);
        activate_saved(&mut c);
        assert_eq!(
            current_setting(&c),
            json!({"profile":"standard","threads":0})
        );
        c.data["performance"]["threads"] = json!(4);
        remember_current(&mut c);
        c.data["transcription"]["model"] = json!("base.en");
        activate_saved(&mut c);
        assert_eq!(
            current_setting(&c),
            json!({"profile":"adaptive","threads":8})
        );
        assert_eq!(
            c.data["performance"]["by_model"][crate::engine::CANARY_MODEL],
            json!({"profile":"standard","threads":4})
        );
        assert!(c
            .changed(&json!({"performance":{"by_model":{
                "parakeet-unified-en-0.6b":{"profile":"adaptive","threads":8}
            }}}))
            .is_err());
    }
    #[test]
    fn cancellation_kills_and_reaps_child() {
        let root = tempfile::tempdir().unwrap();
        let c = Config::at(root.path()).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(measure(Path::new("/bin/sleep"), &c, 5, &cancel)
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
    }
}
