# Speech inference experiments

These are isolated research drivers, not installed application changes. The application, saved configuration, model cache, and history are preserved. The Fire UI work was not modified.

The duration comparison is in [DURATION_REPORT.md](DURATION_REPORT.md). Raw per-call times, transcripts, process memory, and GPU memory are retained in `duration/`. Earlier short-clip and accuracy experiments are in `parakeet/evidence/` and `whisper/evidence/`.

Hardware: Intel i5-12500H with Iris Xe integrated graphics. Models: Canary 180m Flash Q8, Parakeet Unified English 0.6b Q8, and Whisper base.en INT8 with beam size 5 and VAD enabled. Whisper results apply to this cached model, not every Whisper size.

Parakeet uses the existing transcribe.cpp Vulkan backend. Canary uses the GPU encoder and CPU decoder prototype documented in [the Canary report](../canary_bench/FINAL_REPORT.md). Whisper retains CTranslate2, uses eight P-core hardware threads, skips provably empty FFT frames, and limits encoder context to ten seconds only for speech segments at most five seconds long. Longer segments retain the original thirty-second context. This context change still needs broader accuracy testing before adoption.

Earlier checks on 24 complete public speech clips from 13 speakers found Parakeet 2.14x faster with identical output. Whisper was about 4.06x faster on those short clips, with matching normalized words. Shorter Whisper padding settings produced serious hallucinations and were rejected. A longer-clip regression caused the final five-second gate to be added. The FFT-only change was bit-identical in 20 numerical cases. These are small experiments, not general quality guarantees.

## Reproduction

The Cargo manifests and lockfiles are snapshots of the isolated build setup. They contain this machine's absolute paths. Copy each `driver.rs` to `src/main.rs` in its scratch directory before building; do not add these packages to the application workspace.

- CPP driver: `/tmp/parakeet-research`, native source at `/tmp/canary-vulkan-research/native`. The native patch and SDK setup are in `../canary_bench/intel_gpu_experiment/`. Use its setup first on a fresh machine, then replace the driver with this folder's `parakeet/driver.rs`. Both drivers produce the same binary name, so building the old driver afterward will overwrite the new one. Build with the Vulkan CMake settings recorded in `build_drivers.sh`.
- Whisper driver: `/tmp/whisper-research`, linked to this repository's public service APIs. Use the recorded Cargo manifest, lockfile, and current `whisper/driver.rs`. Its binary is in the application target directory but is a separate executable.
- `python3 duration/run.py` runs all 30 model/duration/variant cases sequentially, three transcriptions each. It requires the above built binaries and cached models. It writes results beside the script unless `DURATION_BENCH_DIR` overrides the directory. Audio files must be present in that directory. Existing evidence files will be overwritten, so copy the duration directory for a new run.

`duration/results.json` records the exact invocation and flags for every case. The first transcription warms up the model; the reported time is the median of the next two. RAM includes loading and all three calls. GPU memory is sampled every 100 ms from deduplicated DRM clients; it is system RAM on this integrated GPU and is reported separately from process RSS.

All five required application checks passed: Rust tests, strict Clippy, Python tests, release build, and native probe. Native settings, history, narrow layout, and recording overlay screenshots were inspected. No prototype was installed.
