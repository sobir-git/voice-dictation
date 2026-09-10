# Architecture-specific optimization results

2026-09-10. We built and measured specialized CPU kernels and execution changes.
No variant achieved 2× faster transcription. The most useful result was the
simplest change: use direct depthwise convolution for Canary's two frontend
subsampling layers. It improved speech inference by 1.07–1.12× and reduced peak
process memory by about 26% on this laptop.

## Full inference, profiling removed

Warm medians in seconds, four samples per cell. Each variant ran in two
processes, with three rounds per process and round zero excluded. Process order
was baseline → direct convolution → combined, then the reverse. Audio, Q8
weights, English settings and chunk boundaries were identical. All variants
used the same experimental executable with their switches disabled/enabled.

| Fixture duration | Baseline | Direct convolution | Combined kernels | Direct speedup | Combined speedup |
| --- | ---: | ---: | ---: | ---: | ---: |
| Short, 10.32 s | 0.621 | 0.552 | 0.544 | 1.12× | 1.14× |
| Medium, 31.05 s | 1.744 | 1.634 | 1.589 | 1.07× | 1.10× |
| Long, 134.58 s | 7.631 | 6.984 | 6.831 | 1.09× | 1.12× |
| Technical, 17.20 s | 0.989 | 0.884 | 0.952 | 1.12× | 1.04× |
| Silence, 3.00 s | 0.121 | 0.100 | 0.090 | 1.21× | 1.34× |

Combined means direct convolution plus fused layer normalization and fused
Q8 matrix-vector multiplication with bias. The combined candidate was not
uniformly better than direct convolution alone. Small timing differences are
subject to laptop load, frequency and temperature; these are screening results,
not confidence intervals from a large controlled benchmark.

Peak process RSS was 698.2 MiB for baseline, 519.4 MiB for direct convolution,
and 519.6 MiB for combined. Loading remained approximately 0.12 seconds with
filesystem caches intact. Each process used less than 0.001 CPU seconds during
the final two-second idle check.

Across all 90 transcription calls, every candidate transcript matched its
baseline fixture exactly. Long-recording endings remained present, and silence
remained empty. These fixtures are synthetic and some passages overlap. They
do not establish general human-speech accuracy or equivalence on other languages.

## What the custom work taught us

- The new four-row Q8 dot-product kernel produced bit-identical outputs for
  tested widths 512, 1024 and 4096. In the final isolated kernel probe it was
  about 1.17–1.22× faster than separate row dot products. That did not produce
  a clear full-inference gain in the first screen. Small, repeatedly reused
  matrices exercise different cache and thread behavior from a whole model.
- The fused layer-normalization graph also produced bit-identical outputs for
  all tested shapes. Its isolated operation was faster, but this accounts for
  too little of total inference to deliver a large end-to-end improvement.
- Fusing Q8 matrix-vector multiplication with its bias passed bitwise output
  comparisons across the tested shapes at 1, 2 and 8 threads. Its full-inference
  screen was essentially tied with baseline. Static row assignment and less
  synchronization were insufficient for a substantial gain here.
- Reusing decoder graphs in small growing cache buckets preserved the medium
  transcript but did not clearly improve its runtime. That result does not
  rule out a better implementation; it rejects this particular shortcut.
- Using 1 or 2 threads only for single-token decoding was slower. Four threads
  was only slightly faster in the screen, without enough separation from drift
  to justify changing the default.
- Avoiding im2col in the frontend depthwise convolutions delivered the clearest
  useful gain and the large memory reduction. This reuses an existing native
  kernel, with a Canary-specific choice of where to apply it.

The profile attributed about 66% of operator wall time to matrix multiplication
and 7% to attention. A 2× matrix-kernel gain alone would imply about 1.5× overall
with the rest unchanged. The measured evidence does not support promising a
2× result from a full CPU runtime rewrite.

## Recommendation

Keep the direct-convolution change as the first candidate for broader testing.
It is much smaller than maintaining custom kernels and delivers most of the
observed benefit. Before enabling it in the app, compare public human speech
and the model's supported languages, inspect intermediate tensor drift, and
exercise cancellation and error handling. The current experiment deliberately
keeps all changes outside the live app.

For a larger speed target, prioritize an Intel GPU experiment or a streaming
model comparison. Further CPU work should start with packing/tuning the large
encoder matrix shapes and measuring memory bandwidth and thread stalls when
profiling access is available. The current data does not justify rebuilding
model loading, tokenization and the entire inference engine.

## Reproducibility and checks

See [README.md](README.md) for isolated build, kernel verification and benchmark
commands. [native.patch](native.patch) contains the opt-in implementations;
[profiling.patch](profiling.patch) adds the separate timing instrumentation.
Both patches were checked against the pinned 0.2.3 source. The final numerical
probe passed with byte-identical Q8, layer-norm and matrix+bias outputs.
A final eligibility-guard tightening only rejects non-vector affine shapes;
all benchmark shapes still take the same paths, and numerical probes were rerun.

[evidence/validation-summary.json](evidence/validation-summary.json) contains
unrounded numbers, individual warm samples, text-equivalence checks and memory
measurements. Raw transcripts and timing logs are in
[evidence/validation](evidence/validation). Earlier screens and the operator
profile summary are retained alongside them.
