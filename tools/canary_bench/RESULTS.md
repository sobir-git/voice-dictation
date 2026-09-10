# Canary CPU comparison on this laptop

Keep transcribe.cpp. Handy's ONNX backend did not win this comparison.
No inference backend or user configuration was changed for this benchmark.

Measured on 2026-09-09, Intel Core i5-12500H, Linux x86-64. Both used CPU inference
and 8-bit Canary 180M Flash weights. The ONNX library version, model archive,
and loading calls match Handy at commit `1cd92662805790da9c81639724c3eee5945211bc`.

## Brief decision note

On this laptop, keep the current Canary backend. In the isolated CPU benchmark,
transcribe.cpp took about 10 seconds for 135 seconds of synthetic speech versus
20 seconds for Handy's ONNX path, with 699 versus 1,429 MiB peak memory.
These figures do not establish reliability on arbitrary human speech.

Handy already supports transcribe.cpp, including custom Canary GGUF models.
Its built-in Canary 180M Flash entry still downloads ONNX weights and loads
`CanaryModel` through transcribe-rs. Verified against current main on 2026-09-09:
[built-in entry](https://github.com/cjpais/Handy/blob/1cd92662805790da9c81639724c3eee5945211bc/src-tauri/src/managers/model.rs#L1008),
[ONNX loading](https://github.com/cjpais/Handy/blob/1cd92662805790da9c81639724c3eee5945211bc/src-tauri/src/managers/transcription.rs#L682),
[custom GGUF routing](https://github.com/cjpais/Handy/blob/1cd92662805790da9c81639724c3eee5945211bc/src-tauri/src/managers/model.rs#L1690).

[Handy issue #1961](https://github.com/cjpais/Handy/issues/1961) reports deterministic
empty transcripts for some Canary GGUF audio cut points. The maintainer reproduced
the problem and described it as a model issue; ONNX behaved differently on those
samples. Our synthetic checks do not rule out that failure. Do not recommend
switching Handy's default solely from these speed measurements. No upstream issue
was opened after verifying that transcribe.cpp support already exists.

## Repeated dictation latency

Median of four warm runs per fixture, across two fresh processes per backend.
Each process also performed an initial round excluded from these medians.
The second comparison reversed backend order. The generated audio hashes
matched between comparisons.

| Audio | transcribe.cpp Q8 | Handy ONNX INT8 | ONNX / current time |
|---|---:|---:|---:|
| 10.3 s short dictation | 0.74 s | 1.16 s | 1.55× |
| 17.2 s technical dictation | 1.17 s | 2.03 s | 1.73× |
| 31.0 s dictation | 2.18 s | 4.27 s | 1.96× |
| 134.6 s dictation | 9.96 s | 19.89 s | 2.00× |
| 3.0 s silence | 0.15 s | 0.08 s | 0.52× |

ONNX was faster only on silence. Both returned empty text for that fixture.
Both preserved all 12 topic markers and the final "yellow umbrellas" phrase
in the long recording, without inference errors. Transcripts were identical
across repeated runs within each backend.

## Memory, loading, and accuracy

| Measurement | transcribe.cpp Q8 | Handy ONNX INT8 |
|---|---:|---:|
| Peak process resident memory | 699 MiB | 1,429 MiB |
| Model loading, filesystem cache intact | 0.14–0.30 s | 0.48–0.51 s |
| Word errors / reference words | 38 / 593 | 45 / 593 |
| Weighted word error rate | 6.41% | 7.59% |

Both consumed less than 0.001 CPU seconds in the final two-second idle check.
Model files occupy about 208 MiB for GGUF and 203 MiB for ONNX, excluding the
ONNX download archive. Similar weight sizes did not mean similar runtime memory.

## Scope and limitations

These measurements compare the backends with the same samples, default CPU
settings, English language hint, and the app's quiet-boundary chunking at no
more than 30 seconds. They do not compare complete desktop applications, GPU
inference, or alternative thread tuning. Download, audio decoding, normalization,
VAD, UI work, and typing are outside the inference timing.

This is synthetic eSpeak speech, not a human-speech accuracy benchmark. The
short and medium texts also occur in the long fixture, so the 593 reference
words are not independent test material. The error difference is evidence
against an accuracy benefit on these clips, not a general accuracy ranking.

The laptop remained in normal use. Timing varied, especially in the first
comparison, so the reverse-order run was used to check the decision. Even the
slowest warm GGUF long run, 14.98 s, was faster than the fastest ONNX long run,
18.51 s. Loading times are not cold-boot measurements.

## Reproduce and inspect

[README.md](README.md) contains commands, versions, model sources, and the
verified ONNX archive checksum. [results.json](results.json) contains the
individual warm timings, summaries, and synthetic transcripts. Raw timing
JSONL, hardware metadata, and native stderr from this session are saved locally
under `artifacts/canary-backend-benchmark/forward` and `reverse`.
