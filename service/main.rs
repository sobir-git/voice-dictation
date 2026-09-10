use anyhow::{Context, Result};
use voice_dictation::{
    config::Config,
    daemon::Daemon,
    engine::{read_audio, Engine},
    logging,
};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--adapter") {
        return voice_dictation::adapter::run();
    }
    if let Some(index) = args.iter().position(|s| s == "--benchmark-case") {
        return voice_dictation::optimization::child(
            std::path::Path::new(args.get(index + 1).context("Missing benchmark request")?),
            std::path::Path::new(args.get(index + 2).context("Missing benchmark output")?),
        );
    }
    let config = Config::load()?;
    logging::init(&config)?;
    if voice_dictation::optimization::cli(&args, &config)? {
        return Ok(());
    }
    if let Some(index) = args.iter().position(|s| s == "--transcribe") {
        let path = std::path::Path::new(
            args.get(index + 1)
                .context("--transcribe requires an audio file")?,
        );
        let samples = read_audio(path, config.flag("audio", "preprocess"))?;
        let started = std::time::Instant::now();
        let mut engine = Engine::load(&config)?;
        let loaded = started.elapsed();
        let text = engine.transcribe(&samples, &config)?;
        println!(
            "{}",
            serde_json::json!({"text":text,"load_seconds":loaded.as_secs_f64(),"transcribe_seconds":started.elapsed().as_secs_f64()-loaded.as_secs_f64()})
        );
        return Ok(());
    }
    Daemon::new(
        config,
        args.iter().any(|a| a == "--probe"),
        args.iter().any(|a| a == "--probe-tray"),
    )?
    .run()
}
