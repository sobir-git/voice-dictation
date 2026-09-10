# Duration and RAM comparison

Measured on Intel i5-12500H and Iris Xe. Gains vary with duration and content. These five exact-duration clips are prefixes of one synthetic recording. This tests scaling, not broad recognition accuracy. No user audio was used.

Each baseline/candidate/duration uses a fresh process, one warmup transcription and two measured transcriptions. Times are the median of those two warm calls, excluding model loading and audio capture. Baseline always runs before candidate, so workstation load and run order remain possible sources of noise. Raw sample times are in `duration/results.json`.

RAM columns are MiB. Baseline and candidate process values are peak resident process RAM across loading and all three calls. GPU values are peak resident DRM buffers sampled every 100 ms, with duplicate client IDs removed. GPU buffers use system RAM on Iris Xe and are mostly absent from process RSS. Keep these measures separate: peaks can occur at different times and shared mappings may overlap. This excludes the GUI and whole-application overhead. RAM was measured, not optimized.

## Canary 180m Flash Q8

| Clip | Baseline time | Candidate time | Speedup | Baseline process RAM | Candidate process RAM | Candidate GPU RAM | Same words? |
|---:|---:|---:|---:|---:|---:|---:|:---:|
| 2 s | 0.137 s | 0.194 s | 0.71x | 404 | 237 | 248 | Yes |
| 5 s | 0.293 s | 0.239 s | 1.22x | 445 | 238 | 268 | Yes |
| 15 s | 0.942 s | 0.469 s | 2.01x | 587 | 243 | 333 | Yes |
| 30 s | 2.384 s | 1.280 s | 1.86x | 799 | 253 | 431 | Yes |
| 60 s | 4.776 s | 2.410 s | 1.98x | 756 | 258 | 405 | Yes |

Baseline is CPU with eight threads. Candidate uses Vulkan encoder, CPU decoder, the direct pre-depthwise prototype, F32 Vulkan operations, and eight P-core hardware threads with OpenMP workers paused after each call. Both retain quiet-boundary chunking at at most 30 seconds. A 60-second clip can use less peak RAM than the 30-second clip because the chosen quiet boundaries make its largest chunk shorter. The candidate is slower on the two-second median in this run, with substantial timing variation.

## Parakeet Unified English 0.6b Q8

| Clip | Baseline time | Candidate time | Speedup | Baseline process RAM | Candidate process RAM | Candidate GPU RAM | Same words? |
|---:|---:|---:|---:|---:|---:|---:|:---:|
| 2 s | 0.418 s | 0.208 s | 2.01x | 1129 | 163 | 745 | Yes |
| 5 s | 2.018 s | 0.784 s | 2.57x | 1150 | 163 | 765 | Yes |
| 15 s | 11.138 s | 4.060 s | 2.74x | 1175 | 169 | 782 | Yes |
| 30 s | 23.587 s | 8.509 s | 2.77x | 1177 | 171 | 782 | Yes |
| 60 s | 49.229 s | 15.771 s | 3.12x | 1183 | 176 | 782 | Yes |

Baseline is CPU with eight threads. Candidate uses the existing Vulkan backend with eight threads and unchanged weights. Both use one continuous stream per clip with 5600/1040/1040 ms left/chunk/right context. The benchmark feeds audio without real-time delays, so these are total compute times, not the delay after releasing the recording key. Longer clips are not split into independent 30-second streams.

## Whisper base.en INT8, beam 5, VAD on

| Clip | Baseline time | Candidate time | Speedup | Baseline process RAM | Candidate process RAM | Candidate GPU RAM | Same words? |
|---:|---:|---:|---:|---:|---:|---:|:---:|
| 2 s | 0.554 s | 0.131 s | 4.21x | 281 | 282 | 0 | Yes |
| 5 s | 0.763 s | 0.225 s | 3.39x | 281 | 282 | 0 | Yes |
| 15 s | 2.060 s | 1.801 s | 1.14x | 281 | 281 | 0 | Yes |
| 30 s | 2.770 s | 1.594 s | 1.74x | 282 | 282 | 0 | Yes |
| 60 s | 3.492 s | 3.916 s | 0.89x | 296 | 296 | 0 | Yes |

Baseline is the application CTranslate2 path with four CPU threads. Candidate uses eight P-core hardware threads and skips FFTs for empty padding. Only speech segments of at most five seconds use the shorter ten-second encoder context; longer segments retain the original thirty-second context. Both retain beam size 5, INT8 and VAD. Thus the large short-clip gain should not be expected for 15–60 seconds. Neither uses the Intel GPU.

## Limits and evidence

Short Canary and longer Whisper measurements are noisy. Canary at two seconds measured 0.268 and 0.119 seconds in the candidate, versus 0.138 and 0.136 in the baseline. Whisper at sixty seconds measured 3.402 and 4.429 seconds in the candidate, versus 3.467 and 3.517 in the baseline. Treat the displayed ratios as this run’s rough results, not stable performance guarantees. In particular, there is no demonstrated long-clip Whisper win across all durations.

Matching words means lowercase alphanumeric token sequences agree between baseline and candidate. It does not establish correctness against a reference. Inspect the saved transcripts before adoption; earlier human-speech results and rejected Whisper padding settings are summarized in [README.md](README.md).

The candidate implementations remain isolated prototypes. The required application tests, Clippy, Python checks, release build and native probe passed; selected native screenshots were inspected. No application configuration or installed engine was changed.

Raw evidence: [results.json](duration/results.json), per-call JSONL and stderr files in `duration/`, and [fixture hashes](duration/fixtures.json). GPU accounting follows the kernel [DRM usage-statistics interface](https://www.kernel.org/doc/html/v6.9/gpu/drm-usage-stats.html).
