# Agent interface for inference tuning

Developers extending these capabilities should start with the
[optimization development guide](optimization-development.md).

The product supplies composable commands, not an automatic tuner. Use the existing model cache and native engines. Do not introduce a second inference runtime or duplicate resident models just to improve a benchmark score.

## Command contracts

| Command | Input | stdout | Side effects |
|---|---|---|---|
| `--list-optimizations` | None | JSON capabilities, profile scope, compiled backend availability and CPU information | No model load |
| `--performance-status` | None | JSON daemon state, configured model/performance and actual runtime profile | Starts daemon if needed |
| `--benchmark --profile ID --threads N --seconds 5,15` | Optional `--model NAME`; omitted values use current config | JSON paired report, schema version 1 | Temporarily unloads daemon engine, pauses dictation, runs isolated cases, restores engine, saves report; never saves configuration |
| `--benchmark-results` | None | JSON array of up to eight latest saved reports | Read-only |
| `--apply-optimization ID --threads N` | Optional `--model NAME` | JSON save acknowledgment | Saves that exact model's setting through the daemon; reload occurs in the background |
| `--benchmark-case REQUEST.json RESULT.json` | JSON object containing `config` sections and `seconds` | No stdout; JSON result written to output path | Loads only the requested model in this process; no daemon, configuration, history or output mutations |

All commands are on `target/release/speech-service`. Nonzero exit means failure, with details on stderr. Paired benchmark progress also goes to stderr; stdout stays valid JSON. Ctrl-C or a disconnected requesting client cancels a paired benchmark. Each case has a five-minute watchdog. A GPU failure is never relabeled as a successful CPU benchmark.

An atomic case can be composed into a custom experiment. Example input:

```json
{"config":{"transcription":{"model":"base.en","compute_type":"int8","language":"en","beam_size":5,"vad_filter":true},"performance":{"profile":"fast","threads":4}},"seconds":5}
```

A case uses one warmup and two measured repetitions. Process memory is peak RSS; a raw case's GPU counter is a post-call resident snapshot. The paired runner additionally samples GPU residency every 100 ms. `gpu_memory_measurement` distinguishes them. If orchestrating cases yourself, stop competing inference first and restore it afterward; the raw case intentionally does not manage daemon lifecycle. The paired command handles this coordination automatically.

## Compose an experiment

1. Query capabilities and status. Preserve the user's existing model, precision, language, beam size, VAD and performance settings. Identify whether the installed executable actually includes the requested backend.
2. Start with the existing CPU build. Build `--features vulkan` only when testing a compatible GGUF GPU path. The GPU-enabled executable replaces the CPU executable; no second copy needs to remain installed. Development SDKs belong in the build environment, not the product installation.
3. Compare a small set of thread counts that make sense for this host. Do not copy another machine's affinity masks or assume Intel P/E core numbering. Begin with five-second tests; use 2/15/30/60 seconds when length scaling matters.
4. Keep the transcription settings fixed while comparing performance. Inspect `rows[].reference` and `candidate` timings, `same_words`, `stable_words`, both texts, loading time, and process/GPU memory. Repeat close or noisy results. Normalized text agreement on the synthetic English fixture is not evidence of broad recognition accuracy.
5. Decide using the user's actual workload. The experimental base.en adaptive profile changes context length while retaining one loaded model. Do not promote it solely on synthetic timing or apply it to untested model sizes. Full GPU versus CPU switching per clip would require accounting for model reload time or duplicate RAM; neither is silently enabled.
6. Apply the selected profile explicitly. This records the choice under the exact model ID, so later model switches restore it automatically. Poll `--performance-status` until `model_ready` is true. Inspect `runtime_profile`: ordinary dictation may have fallen back to CPU if GPU initialization failed. Restore previous settings if verification fails. A save acknowledgment alone is not proof that acceleration is active.

Raw reports are local under `~/.local/share/speech-to-text/benchmarks/`. There is no telemetry. Research folders are not installation dependencies. Keep installed artifacts limited to the chosen build and existing required libraries/models.

Default builds retain the existing CPU dependencies plus approximately 1 MB of bundled synthetic speech. Vulkan currently adds roughly 40 MB, so it is opt-in. For prebuilt releases, configure native CPU ISA flags for the supported host floor; locally native-tuned binaries are not universal binaries. See [performance experiments](performance-lab.md) for the available profiles and build requirements.

History rows also expose `model` and `transcription_seconds`. These describe the latest completed or failed attempt, including retries. Transcription time is measured from recording stop or retry request through queueing, any subsequent model loading, audio preparation and completed inference. It excludes spoken recording time and text insertion. Older records retain null metadata. This end-to-end wait is distinct from warm inference time in benchmark reports.
