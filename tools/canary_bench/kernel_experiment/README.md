# Canary CPU kernel experiments

Isolated experiments on 2026-09-10, using transcribe.cpp 0.2.3, Canary-180M Q8,
Intel i5-12500H and Linux. These patches are opt-in research code. The app and
its production dependencies do not use them.

## What was built

| Variant | Implementation | Hypothesis |
| --- | --- | --- |
| `q8x4` | New AVX2 four-row Q8 dot-product kernel, sharing activation loads and scales | Less redundant work in single-token matrix-vector products |
| `gemv` | Fused Q8 matrix-vector product and bias, using the new kernel | Avoid an intermediate tensor, a separate bias pass and a synchronization point |
| `ln` | Fused layer normalization, per-channel multiplication and bias | Keep each output row in cache and remove two synchronization points |
| `bucket` | Reuse decoder graphs in growing 32, 64, 128... token-cache buckets | Amortize graph construction without always attending over 1024 positions |
| `dw` | Use the existing direct depthwise convolution for the two frontend downsampling convolutions | Avoid materializing convolution input patches with im2col |
| `step1/2/4` | Set single-token decoder threads independently of encoder/prefill threads | Small token operations might benefit from less thread coordination |
| `combined` | `gemv` + `ln` + `dw` | Check whether individually small gains accumulate |

All variants use the same Q8 weights, audio samples and chunk boundaries.
The four-row dot product preserves each row's multiplication, scaling and
accumulation order. Layer-norm fusion also preserves the original rounding
steps. Fusion checks require compatible F32/one-dimensional affine tensors and
GGML's single-consumer eligibility checks. Unsupported shapes use the original
operations. CPU SIMD paths are guarded by AVX2 at compile time.

The direct depthwise path uses F32 kernels and can differ numerically from the
original path. Transcript equality on these fixtures is a test result, not a
proof that every recording will produce identical output. The decoder bucket
experiment preserves the causal mask and retains the separate KV allocation
when rebuilding a larger graph. It has not been tested across all cancellation,
translation, backend and model combinations.

## Why these experiments

`perf record` was blocked by the machine's performance-event policy. We left
that policy unchanged and instrumented an isolated copy of GGML instead.
The first profile attributed approximately 66% of measured operator wall time
to matrix multiplication, 9% to im2col, 8% to addition, and 7% to attention.
These measurements include synchronization and profiler disturbance, and
aggregate cold and warm runs. They guide experiments; final performance
measurements run with the profiling code removed.

The largest decoder matrix shapes were Q8 1024×1024, 1024×4096 and 4096×1024
multiplied by one token vector. Existing CPU fusion handled RMS normalization,
but Canary uses layer normalization with separate affine operations. Existing
Q8 kernels already use AVX/VNNI where available. We are not adding a missing
SIMD compiler flag.

The experiments apply the same general ideas as operator fusion and
hardware-specific scheduling in [TVM](https://arxiv.org/abs/1802.04799) and
[oneDNN's matrix fusion patterns](https://uxlfoundation.github.io/oneDNN/v3.9/dev_guide_graph_matmul_fusion_patterns.html).
They are small hand-written prototypes; neither framework was integrated.
[FlashAttention](https://arxiv.org/abs/2205.14135) avoids attention intermediates
through tiling, but our installed Canary already uses GGML flash attention.
With only about 7% attributed to that operation here, it was a weak first target.

## Reproduce

Requires the base benchmark's dependencies plus a C++ compiler and Git. Run
from this repository. `prepare` copies the pinned crate into a fresh temporary
directory, applies the experimental patch, and builds there. It never modifies
Cargo's registry source, the app checkout's dependencies or cached model files.
The initial isolated build updates its own lockfile for the local crate patch.

```sh
python3 tools/canary_bench/kernel_experiment/run.py prepare \
  --work /tmp/canary-kernel-reproduction
python3 tools/canary_bench/kernel_experiment/run.py probe \
  --work /tmp/canary-kernel-reproduction
python3 tools/canary_bench/kernel_experiment/run.py bench \
  --work /tmp/canary-kernel-reproduction \
  --gguf /path/to/canary-180m-flash-Q8_0.gguf \
  --fixtures /path/to/base-benchmark-fixtures \
  --variants baseline,dw,combined --orders 2
```

The fixture directory is produced by [the base benchmark](../README.md).
`bench` runs three rounds per process, excludes round zero from warm medians,
and reverses variant order in the second pass. It saves complete transcripts,
stage timing, CPU time, peak RSS and idle CPU measurements. Speed alone does
not establish correctness; compare the saved text as well.

To examine operation timing, prepare another fresh directory with `--profile`
and pass `--profile` to `bench`. This applies `profiling.patch` after
`native.patch`. Never compare profiled timings with unprofiled timings.

`probe` compares four-row Q8 dots against the original dot-product function for
inner widths 512, 1024 and 4096. It also compares full fused/unfused graphs for
layer-norm widths 512 and 1024 with 1, 9, 71 and 318 rows, and matrix+bias
shapes across 1, 2 and 8 threads. The sampled inputs are deterministic Gaussian
values. Raw outputs must match byte for byte. This is a numerical regression
screen, not exhaustive floating-point validation. Its tiny-graph timing includes
threadpool/graph execution overhead, so use it to explain mechanisms, not to
predict total dictation speed.

## Evidence

`evidence/` contains screening transcripts and timings, the profile summary and
kernel checks. `RESULTS.md` records the final paired comparison. Full working
files and logs are also retained in `/tmp/canary-kernel-research/` locally.

The experiments do not establish that writing a whole inference runtime would
be faster. A 2× speedup of the matrix operations alone would imply only about
1.5× overall at the measured fraction. We would need gains across several
parts of execution, or substantially faster hardware/model architecture, to
reach 2× reliably.
