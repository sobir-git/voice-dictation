# Intel GPU experiment and fast iteration loop

Isolated experiment, 2026-09-10. The app is unchanged. Nothing here establishes
a complete 2× inference improvement. Multi-token prediction is separate.

Hardware: Intel i5-12500H, Iris Xe ADL GT2, Mesa 25.2.8. Packed 4×8-bit integer
dot products are accelerated; cooperative matrix operations are unavailable.
The original Ubuntu shader compiler did not support the integer-dot extension.
The isolated LunarG 1.4.357.1 compiler enables that path.

## Where iteration time went

A clean native build took 134 seconds. Editing a shared shader include caused
roughly 60 seconds of rebuilding. The initial 25-variant tile sweep also
repeated model setup, full transcription and a two-second idle check for every
variant. This was too expensive for individual kernel edits.

The new loop builds ggml once with an opt-in external shader loader, then:

1. Edit shaders in `/tmp/canary-vulkan-research/shader-edit/`.
2. Run `python3 /tmp/canary-vulkan-research/compile_shader.py`.
3. Run `matrix-probe` with `CANARY_VK_SHADER_DIR` pointing to `shader-bin/`.
4. Compare output values before ranking candidate timings.
5. Run a fixed five-second synthetic clip for routine full-transcription timing.
6. Reserve longer clips and human-speech checks for a candidate worth adopting.

Measured two-shader compilation took 0.217 seconds. Each four-shape matrix
probe took 0.19–0.32 seconds, including process startup. This is iteration
latency, not an inference speedup. Compiler caches, machine load and shader
complexity will affect future runs.

The probe covers Q8 matrix multiplication plus bias for `(K,M)` equal to
`(1024,1024)`, `(1024,4096)`, `(4096,1024)`, and `(1024,5248)`. It warms each
shape, records 25 calls and writes output floats for comparison. These are
hot isolated operations; full decoding rereads many weight matrices, so a
microbenchmark win is only a screening result.

The external shader matched the embedded shader bit-for-bit on these inputs.
Separating Q8 integer weights from their scales also matched bit-for-bit with
the original tile shape. Changing the reduction tile produced a maximum
absolute difference of 1.29e-5. Speech accuracy still needs separate checking.

`BENCH_SKIP_IDLE=1` skips the two-second idle measurement only during screening.
Final candidate validation must retain idle, load, memory and full-pipeline
measurements. Benchmark processes run sequentially to avoid GPU contention.

## Reproduction files

- `native.patch` applies to transcribe-cpp-sys 0.2.3 in an isolated copy. It adds
  opt-in tile settings, Q8 weight packing, and external shader loading.
- `tile.patch` contains the earlier tile-only experiment.
- `compile_shader.py` compiles the two Q8 integer-dot shader variants.
- `matrix_probe.cpp` exercises the dominant decoder matrix shapes.
- `driver.rs` is the standalone full-transcription driver.
- `convert_f16.py` creates separate files by expanding Q8 weights to FP16.
  It checks every tensor's dimensions and content after conversion. The
  original model is retained. FP16 expansion adds rounding and needs quality
  validation; it does not recover the original pre-quantization weights.
- `evidence/` contains raw timing records and iteration-loop checks.

The scripts currently use the explicit scratch directory above. The compiler
and library dependencies are extracted there, without changing system packages.
Use the same layout or adjust the root path in the scripts.

## Prototype limits

The packed-weight cache assumes immutable two-dimensional Q8 weights on this
host-coherent integrated GPU. It retains a second copy of those weights and
keys the experimental cache by tensor address. Production integration needs
ownership-aware invalidation, memory accounting and an upload lifecycle that
also supports non-mapped device memory. Do not copy this cache into the app.
The external shader loader accepts local executable shader code and belongs
only in the isolated research build.

## Sources behind the experiments

[Intel Xe-LP optimization guide](https://www.intel.com/content/www/us/en/developer/articles/guide/lp-api-developer-optimization-guide.html)
describes memory coalescing and SIMD-width tradeoffs. The tile sweep tests those
choices on this machine rather than assuming a width is best.

[llama.cpp SYCL Q8 reorder proposal](https://github.com/ggml-org/llama.cpp/pull/21527)
separates scales from integer weights and reports a 3.1× decoding gain on a
newer Arc Pro B70 with a large language model. That supports testing the layout;
it is not evidence of that gain on Canary, Vulkan, or Iris Xe.

## Routine transcription screen

The user's requested default is five seconds of synthetic speech, 16 kHz mono
float32, stored at `/tmp/canary-vulkan-research/five-second-screen.f32`.
It is the first 320,000 bytes of the existing synthetic `short.f32` fixture.
`screen.py` runs three calls per variant, discards the first for warm timing,
and skips the two-second idle check. The cut point may split a word, so this
fixture tests latency and output stability, not word error rate.

Long clips and broad accuracy runs are acceptance checks for a finalist, not
part of each shader iteration.

Latest shader experiment: `block32-shader.patch` applies to the editable shader
copy after `native.patch`. Each lane processes a full 32-weight quantization
block, accumulating integer dot products before applying the scale once.
The matrix probe's maximum absolute difference from the reference was 1.15e-5.
The five-second clip produced the same transcript on CPU, original GPU,
packed/tuned GPU and the slower FP16 expansions. These checks cover only this
screen, not broad speech quality.

Routine `screen.py` defaults to CPU, GPU and the packed/tuned candidate. The
slower FP16 candidates remain available with `--variants f16-decoder,f16-all`
only when a new reason justifies retesting them.

## Further screening, still below 2×

Pinning four CPU inference threads to the four performance cores improved the
five-second Q8 direct-convolution screen to about 206 ms. The default eight
threads with the same convolution change took about 252 ms. This is specific
to the i5-12500H topology: performance-core logical CPUs are 0,2,4,6.
`taskset` applies only to the benchmark child process.

The combined GPU Q8 prototype with direct frontend depthwise convolution took
about 232 ms, versus the earlier default CPU baseline around 288 ms. A tuned
Q4 expansion/requantization screen took about 198 ms but changes precision and
has not passed broad quality checks. None of these is a 2× full-inference win.
A frequency probe observed the GPU reaching its 1300 MHz maximum; it did not
change clocks or power policy.

`CANARY_HYBRID=1` is an isolated GPU-encoder/CPU-decoder prototype. It copies
decoder weights to CPU memory once, gives decoding a CPU-only scheduler and KV
cache, and transfers encoder output through the existing host handoff. It
currently recreates scheduler scratch between stages and retains unused space
in the original GPU weight buffer. This is experimental ownership and memory
management, not production code.

## Final bounded comparison

The [final report](../FINAL_REPORT.md) supersedes exploratory timing figures
above: 1.97× on nine mostly five-second clips, 1.76× on 31 complete short
utterances, with unchanged Q8 weights and matching paired transcripts.
This does not establish a reliable general 2× result. CPU tuning alone gave
1.44× on the fixed set.

Winning settings on this machine:

```text
BENCH_BACKEND=vulkan
BENCH_THREADS=8
CANARY_HYBRID=1
CANARY_DIRECT_PRE_DW=1
GGML_VK_DISABLE_F16=1
OMP_WAIT_POLICY=ACTIVE
BENCH_PAUSE_OMP=1
taskset -c 0-7
```

The Rust driver calls `omp_pause_resource_all(omp_pause_soft)` after each
transcription. This is part of measured latency, including the cost of worker
restart on subsequent calls. Without the pause, active waiting consumed
6.00 CPU-seconds over a two-second idle window. Final candidate idle readings
were below 0.0001 CPU-seconds over two seconds. This is required for any future
integration; setting an environment variable alone is unsuitable for the daemon.
[GCC's waiting-policy documentation](https://gcc.gnu.org/onlinedocs/libgomp/OMP_005fWAIT_005fPOLICY.html)
explains why active waiting consumes CPU.

`screen.py` now defaults to CPU and the two hybrid variants. The older GPU-only
and precision-conversion variants remain optional. Each process has a ten-second
screen deadline. `validate_candidate.py` uses a fifteen-second process deadline,
rotates variant order across three cycles, and excludes round zero from warm
summary values. It preserves raw outputs, errors, model/fixture hashes and idle
readings. Fixed-clip and complete-utterance evidence live in separate folders.

### Reproduce

The build targets this Linux/i5-12500H machine. Install nothing into the app.
The isolated root needs LunarG SDK 1.4.357.1 at `1.4.357.1/x86_64/`, a system
Vulkan loader at `/usr/lib/x86_64-linux-gnu/libvulkan.so.1`, the cached
transcribe-cpp-sys 0.2.3 crate and the cached Q8 model. The SDK archive used here
has SHA-256 `4b41e3b30e8aedaa5dac7c136561ab463eb316a25a54e2c6245f2c299ea1fb85`.
Python fixture preparation requires numpy, pyarrow and soundfile; the isolated
venv contains these dependencies. The existing Ubuntu glslc lacks the required
integer-dot compiler support.

```sh
./tools/canary_bench/intel_gpu_experiment/build_isolated.sh
/tmp/canary-vulkan-research/venv/bin/python tools/canary_bench/intel_gpu_experiment/prepare_fixtures.py
python3 tools/canary_bench/intel_gpu_experiment/validate_candidate.py --idle
python3 tools/canary_bench/intel_gpu_experiment/validate_candidate.py --fixture-set complete-short --variants baseline,hybrid8-f32
```

`build_isolated.sh` verifies an existing patched tree instead of overwriting it.
The manifest and lockfile are preserved. The script was run successfully after
cleanup. All 82 regenerated public/cropped/synthetic audio files matched the
measured fixtures byte for byte. The script's CPU affinity is specific to this
machine; do not copy core numbers to a different CPU topology.

### Rejected kernel changes

The optional float-matrix tile hook preserves the aligned-load specialization
flag. Dropping that flag caused large slowdowns. Smaller tiles improved a
four-shape matrix probe from 3.50 ms to 1.24 ms with identical outputs, but made
full transcription slower. Some subgroup/tile combinations produced incorrect
outputs or timed out; the hook is an experiment, not a safe autotuner.

Pipeline compiler statistics showed 47–77 register spills in several original
FP16/FP32 kernels, while the Q8 integer-dot kernel had none. Those statistics
motivated the tile experiment; they did not prove a full-inference bottleneck.

`native-with-rejected-cpu-experiments.patch` is a separate, complete patch
against the original crate, not an incremental patch over `native.patch`.
It preserves unsigned/reordered CPU Q8 packing and bias/residual store fusion.
The packing implementation requires AVX-VNNI and is a machine-specific research
artifact. Its microprobe matched the reference, but neither packing nor the
fusions gave a reliable complete-transcription gain. They were removed from
the final candidate build. Earlier opt-in CPU experiments remain disabled.

The external `vector128-shader.patch` includes full-32-weight accumulation and
128-bit loads. Apply it to a separate editable shader copy after `native.patch`;
it is an alternative to `block32-shader.patch`, not an additional patch on top.
The final hybrid candidate does not use the external decoder shader.
