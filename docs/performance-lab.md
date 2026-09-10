# Performance experiments

Benchmarks and profile selection are terminal-only. There are no new desktop screens, controls or polling tasks. Users choose their own configurations; the app does not choose a winner automatically.

```sh
# Show what this build supports.
target/release/speech-service --list-optimizations

# Compare this candidate with Standard CPU on the currently selected model.
target/release/speech-service --benchmark --profile standard --threads 2 --seconds 5

# Test Whisper base.en's adaptive context at the five requested lengths.
# This does not change the selected model or saved settings.
target/release/speech-service --benchmark --model base.en --profile adaptive --threads 4 --seconds 2,5,15,30,60

# GPU-enabled builds can test the selected GGUF model.
target/release/speech-service --benchmark --profile vulkan --threads 4 --seconds 5,15

# Canary can split its encoder and decoder across the best backend for each.
target/release/speech-service --benchmark --model canary-180m-flash --profile hybrid --threads 8 --seconds 2,5,15,30,60

# Only this separate command applies a choice to the daemon and config.yaml.
target/release/speech-service --apply-optimization fast --model base.en --threads 4

# Return to the existing defaults, or inspect previous reports.
target/release/speech-service --apply-optimization standard --threads 0
target/release/speech-service --benchmark-results
```

The default test length is five seconds. Ctrl-C cancels a benchmark; closing its connection also cancels it. Long suites can take several minutes. Progress goes to stderr and the completed JSON report to stdout, so reports can be redirected to files or inspected with jq.

Benchmarks use the selected or explicitly overridden model, language, beam size, VAD and performance settings. They do not save settings, insert text, or add recordings to history. The current daemon engine is unloaded before testing and restored afterward. Each reference and candidate uses a fresh process with one warmup and two timed runs. Only one model is resident at a time within the service and its benchmark children. Dictation and settings saves are temporarily unavailable while benchmarking; the desktop remains responsive.

## Available configurations

| Profile | Models | Behavior |
|---|---|---|
| standard | All | Existing full-context CPU inference. Thread count remains adjustable. |
| vulkan | GGUF models | Existing transcribe.cpp GPU backend. Requires a GPU-enabled build and compatible driver. Benchmark failures are reported, not silently measured on CPU. |
| hybrid | Canary 180M Flash | Runs the parallel encoder and pre-encoder depthwise convolution on Vulkan, then moves the autoregressive decoder and KV cache to CPU. Requires a Vulkan build. |
| fast | Whisper | Skips FFT work for zero padding and precomputes window coefficients. Features are numerically identical to the reference implementation. |
| adaptive | Whisper base.en | Faster preprocessing plus ten-second encoder context for post-VAD segments up to five seconds. Longer segments retain the original thirty-second context. One loaded model serves both paths, so there is no second model allocation or switching load. Accuracy can change. |

Zero threads means the engine's existing default. Other values are explicit counts. The Canary hybrid profile detects mixed-core Linux hosts from per-CPU maximum frequencies and limits inference to the faster class when there is a clear split. It makes no affinity change on homogeneous or unreported topologies. OpenMP uses its normal passive idle behavior; an active-wait experiment was faster but kept CTranslate2 workers busy after a model switch. Benchmarks do not change settings. Applying a result records the profile and thread count under the exact model ID. Switching models automatically restores that model's saved setting; a model without one starts on Standard CPU. A saved backend unavailable in the current build falls back to Standard CPU without deleting the saved choice. History retries use the active saved performance settings, or explicit performance overrides supplied over IPC.

The Settings page displays the engine and backend that the daemon actually loaded, such as `CTranslate2 · CPU` or `transcribe.cpp · Vulkan`. If Vulkan loading fails, it displays `CPU fallback`; diagnostics and the service log retain the detailed reason.

The Canary hybrid path uses a small pinned transcribe.cpp fork. It is a separate profile because plain Vulkan remains useful for comparison and other GGUF models. The rejected architecture-specific kernel prototypes remain isolated in the research folder.

## Reading results

Warm time excludes model loading and audio capture. Loading time is reported separately and includes downloading an uncached model on first use. Parakeet is fed as a continuous buffered stream without real-time delays; its measured compute time is not the delay after releasing the dictation key. Two warm repetitions provide a quick comparison, not a statistical guarantee; rerun close results under similar machine load.

RAM is peak process RSS, including loading and warmup. GPU buffers are sampled separately every 100 ms from DRM clients, deduplicated by client ID. Integrated GPUs use system RAM. GPU counters can be unavailable; unknown values are null rather than zero. Do not blindly add peaks, since they can overlap and occur at different times. These measurements exclude GUI overhead.

Both transcripts are included. `same_words` compares normalized candidate text with the CPU reference, not with ground truth. The bundled English synthetic fixture does not establish accuracy for other speakers, languages, noise or long-form recordings. The JSON report also records whether words remained stable across repetitions, raw timings, selected settings, hardware, devices and fixture version.

Results are local JSON files under `~/.local/share/speech-to-text/benchmarks/`. No telemetry or user audio is uploaded. On GPU model-initialization failure, ordinary dictation falls back to CPU and records the reason in diagnostics and the service log. Runtime transcription failures retain the recording for history retry. Benchmark GPU failures remain failures, so they cannot be mistaken for successful GPU measurements.

## Size and building

The runner adds one approximately 1 MB lossless FLAC sample and small Rust code. It uses the existing ffmpeg dependency to decode the sample. It adds no models, Python environment, separate benchmark executable, inference runtime or Vulkan SDK. The service executable doubles as the disposable benchmark worker. Research directories are not runtime dependencies and are not needed in an installation.

The default CPU-only build retains standard, fast and adaptive experiments. GPU support is an optional feature in the same executable, not a second installed engine. The first GPU build measured roughly 40 MB larger than the existing service, which is why it is optional.

```sh
cargo build --release --locked
# Optional GPU build:
cargo build --release --locked --features vulkan
# Or build/install the GPU edition, preserving existing configuration and history:
VOICE_DICTATION_FEATURES=vulkan ./update_local.sh
```

GPU builders need Vulkan development headers, loader and a recent glslc shader compiler, discoverable through the Vulkan SDK/CMake toolchain. End users need the Vulkan loader and their normal GPU driver, not the development SDK. A CPU-only build must not be presented as GPU-enabled.

Native compiler ISA settings must be selected for the release's supported CPU floor when distributing prebuilt binaries. Do not distribute a locally CPU-native-tuned binary as a universal host build. transcribe.cpp exposes `TRANSCRIBE_X86_CONSERVATIVE=ON` for a conservative x86 build through `TRANSCRIBE_CMAKE_ARGS`; release builders can retain their Vulkan SDK arguments alongside it.
