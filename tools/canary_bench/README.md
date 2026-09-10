# Canary backend benchmark

Latest: [bounded Intel GPU research and final measurements](FINAL_REPORT.md).

Compare the app's transcribe.cpp CPU backend with Handy's ONNX CPU backend
without changing the app's dependencies, configuration, history, or recordings.

The driver pins `transcribe-cpp` 0.2.3 and `transcribe-rs` 0.3.8. The latter and
ONNX Runtime's Rust binding, `ort` 2.0.0-rc.12, match
[Handy's lockfile at 1cd9266](https://github.com/cjpais/Handy/blob/1cd92662805790da9c81639724c3eee5945211bc/src-tauri/Cargo.lock).
It uses Handy's `CanaryModel::load(..., Quantization::Int8)` and default CPU
session settings. There is no GPU acceleration or custom thread tuning.

## Run

Requires Rust, a C++ toolchain with OpenMP, Python 3, ffmpeg, and libespeak-ng.

```sh
cargo build --release --locked --manifest-path tools/canary_bench/Cargo.toml
python3 tools/canary_bench/benchmark.py \
  --output /tmp/canary-benchmark-forward \
  --gguf /path/to/canary-180m-flash-Q8_0.gguf \
  --onnx /path/to/canary-180m-flash
python3 tools/canary_bench/benchmark.py \
  --output /tmp/canary-benchmark-reverse --order onnx,gguf \
  --gguf /path/to/canary-180m-flash-Q8_0.gguf \
  --onnx /path/to/canary-180m-flash
```

The GGUF is the app's cached model from
[handy-computer/canary-180m-flash-gguf](https://huggingface.co/handy-computer/canary-180m-flash-gguf).
The ONNX directory comes from
[Handy's model package](https://blob.handy.computer/canary-180m-flash.tar.gz).
Its archive SHA-256 must be
`6d9cfca6118b296e196eaedc1c8fa9788305a7b0f1feafdb6dc91932ab6e53f7`, matching
[Handy's model registry](https://github.com/cjpais/Handy/blob/1cd92662805790da9c81639724c3eee5945211bc/src-tauri/src/managers/model.rs#L1008).
Download and extract it into a temporary directory, not the live app's data directory.

## Measurements

- Five deterministic eSpeak fixtures: short, medium, long, technical, and silence.
  Each fixture is synthesized in a fresh process to isolate eSpeak's callback state.
- Both backends receive exactly the same 16 kHz mono float samples, English language
  setting, and quiet-boundary splits of at most 30 seconds, matching the app's
  Canary long-recording fix. Handy itself passes whole recordings; using the same
  splitting here compares inference backends without giving one a length disadvantage.
- No normalization, VAD, audio decoding, text output, model download, or UI work
  occurs inside the timed inference region.
- One model stays loaded for three rounds. Round zero is reported separately;
  rounds one and two measure repeated dictation. The reverse run checks order effects.
- Linux `getrusage` records CPU time and process peak resident memory. Memory
  includes the same fixture buffers for both backends. A final two-second wait
  measures CPU use after inference.
- Word error rate uses case-insensitive alphanumeric tokens and standard word
  edit distance. It is a check on these synthetic fixtures, not an estimate of
  accuracy on human speech. The long fixture overlaps the short and medium text.

Each output directory contains references, audio, hardware metadata, raw timing
and transcript JSONL, native stderr, and a summary. Model loading is timed with
filesystem caches intact; this does not simulate a cold machine boot.

See [RESULTS.md](RESULTS.md) for the measured decision on this laptop.
See [SPEED_RESEARCH.md](SPEED_RESEARCH.md) for follow-up thread and quantization
experiments, alternative runtimes, and relevant research papers.
