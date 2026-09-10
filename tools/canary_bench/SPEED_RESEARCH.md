# Can we make dictation twice as fast?

Research and isolated CPU experiments, 2026-09-10.

**Update:** the subsequent Intel GPU/hybrid experiments are complete within the
user's time cap. See [the final report](FINAL_REPORT.md): 1.97× on the fixed short
set and 1.76× on complete short utterances. The CPU-only findings below are
historical context.

No tested replacement has delivered a reliable 2× improvement over our current
Canary Q8 backend. I recommend testing Intel GPU acceleration and a streaming
model before investing in a new inference engine. Processing audio while the
user speaks is also a practical way to reduce the wait after releasing the
hotkey, even without reducing total computation.

## What we measured

Intel i5-12500H, Linux, Intel Iris Xe integrated graphics. These experiments used
CPU only, the same synthetic audio and at-most-30-second chunking as
[the earlier benchmark](RESULTS.md). Each process ran three rounds; the table
uses the median of the two warm rounds. This is an exploratory screen, not a
human-speech accuracy evaluation or a thermally controlled performance study.
Compare candidates within this run, not against absolute timings from another
session.

| Experiment | 31.05 s audio | 134.58 s audio | Decision |
| --- | ---: | ---: | --- |
| Current Canary Q8, default CPU threads | 1.709 s | 7.356 s | Reference |
| Same model family, Q4_K_M weights | 1.437 s | 5.798 s | Reject: missing speech |
| ONNX Int8, 2 intra-op threads, sequential execution | 3.303 s | 15.754 s | Slower |
| ONNX Int8, 4 intra-op threads, sequential execution | 2.720 s | 12.714 s | Slower |

Q4's apparent speedup was 1.19× and 1.27×. On the long fixture, word errors rose
from 23/411 to 119/411, or 5.60% to 28.95%, and the final reminder disappeared.
Producing less text can itself shorten decoding time. These numbers do not
establish a useful performance gain. The medium fixture had 11/100 errors with
Q8 and 10/100 with Q4, so checking a short sample alone would have missed this.

A separate thread sweep on the medium fixture produced warm medians of 1.66 s
at the default 8 threads, 2.12 s at 2, 1.77 s at 4, 1.86 s at 6, and 1.60 s at
12. Repeating the default at the end gave 1.72 s. All transcripts matched.
The small apparent advantage at 12 threads needs more testing; it is nowhere
near 2×.

The earlier, more extensive comparison found Handy's default ONNX path roughly
1.6–2× slower on speech and using roughly twice the peak memory. This establishes
the result for those settings, not a limit on every ONNX configuration.
The new ONNX runs patch only `transcribe-rs` session construction to bound
intra-op threads and disable parallel graph execution. Four threads performed
better than two but still took 1.59× and 1.73× the current Q8 time. Other
thread-pool settings and execution providers remain untested. ONNX Runtime
documents thread count, spinning and shared-pool controls, so tuning was worth
checking. [Thread management](https://onnxruntime.ai/docs/performance/tune-performance/threading.html).

## Where the current time goes

Instrumenting `Transcript.timings` on the medium fixture gave about 44 ms of
mel processing, 828 ms of encoding and 759 ms of decoding, averaged over two
warm runs and summed across chunks. Encoding and decoding each consume roughly
half the inference time.

Therefore, even a 2× decoder optimization would give about 1.3× overall speedup
with the encoder unchanged. Claims of several-fold faster decoding are not
automatically several-fold faster dictation.

The installed transcribe.cpp 0.2.3 source already uses native CPU compilation,
OpenMP, tinyBLAS, attention optimizations and cached decoder attention state.
Its CPU decoder rebuilds the computation graph as the token history grows;
the GPU path reuses a fixed-size graph. Profiling graph construction versus
matrix multiplication could identify a useful patch. Copying the GPU strategy
blindly would make the CPU process padded history, which the source explicitly
avoids. The generic speculative-decoding option has no implementation in this
Canary path. These findings came from the installed crate source and generated
compiler flags, not assumptions about the project name.

## Alternatives worth testing

| Route | User benefit if it works | Work and uncertainty |
| --- | --- | --- |
| Existing transcribe.cpp Vulkan backend on Iris Xe | Same model, shorter inference | Small isolated build first. Driver availability alone does not prove a speedup. CPU/GPU transfers and small decoder steps may erase gains. |
| OpenVINO with Canary ONNX | Could improve use of Intel CPU or integrated GPU | Medium prototype. Verify operator support, dynamic shapes and cached decoding. No local Canary result yet. |
| Moonshine v2 streaming | Could sharply reduce waiting after speech ends | Changes the model and its accuracy/language behavior. Needs public human-speech evaluation and adapter work. |
| Optimize current CPU implementation | Preserve the current model and integration | Profile first, then make a narrow upstream patch. No evidence yet of enough removable overhead for 2×. |

transcribe.cpp already offers Vulkan acceleration; trying that is cheaper than
writing another runtime. [Upstream implementation](https://github.com/handy-computer/transcribe.cpp).
OpenVINO exposes latency-oriented CPU/GPU execution settings, but its general
capabilities do not establish Canary compatibility or performance.
[OpenVINO performance hints](https://docs.openvino.ai/2025/openvino-workflow/running-inference/optimize-inference/high-level-performance-hints.html).
The separate Rust project canary-rs also wraps ONNX and labels several optional
execution providers as mostly untested. A different wrapper is not itself a
faster engine. [canary-rs README](https://docs.rs/crate/canary-rs/latest/source/README.md).

Moonshine v2 uses a streaming encoder with local attention. Its reported latency
measurements include Apple M3 tests and measure delay after speech ends; they
are not measurements against Canary on this Intel laptop. It is a promising
candidate to benchmark, not a demonstrated winner here.
[Paper](https://arxiv.org/abs/2602.12241),
[official runtime](https://github.com/moonshine-ai/moonshine).

CTranslate2's supported Transformers include Whisper, but do not list Canary.
Our existing Whisper fallback can be compared as a different model; switching
Canary to faster-whisper is not a drop-in backend change.
[Supported models](https://opennmt.net/CTranslate2/guides/transformers.html).

## Paper techniques

**Token Map Drafting is the first speculative technique I would prototype.**
It drafts tokens from a precomputed domain-specific n-gram map and verifies
them with the full model, avoiding a second neural model. The paper reports
1.27× and 1.37× decoding speedups on structured domains, with unchanged word
error rates, using CPU execution. The experiments target Whisper, so a Canary
port must implement multi-token verification and correct cache rollback.
Based on our stage split, those decoder gains would imply only about 1.1–1.15×
overall, assuming they transfer at all.
[Token Map Drafting, 2025](https://arxiv.org/abs/2507.21522).

**Medusa-style heads are a larger investment.** The Whisper paper reports about
50% latency reduction by training extra token-prediction heads and verifying
their proposals. Canary would need trained heads and inference changes. This
is not a switch we can enable on the downloaded model.
[Whisper in Medusa's Ear, 2024](https://arxiv.org/abs/2409.15869).

**Large ASR language-model results do not transfer directly.** SpecASR reports
roughly 3× acceleration in experiments with a Vicuna-13B target. Our much
smaller Canary model has different bottlenecks and a substantial encoder cost.
[SpecASR, 2025](https://arxiv.org/abs/2507.18181).

**Streaming is the strongest route to a shorter perceived wait.** NVIDIA
documents Canary streaming with Wait-k and AlignAtt policies. AlignAtt uses
cross-attention to decide whether to emit more text or wait for more audio.
Its fixed-context mode can trade accuracy for latency. The documented NeMo
examples are not proof that Canary-180M in transcribe.cpp supports that path;
porting and evaluation would be required.
[NVIDIA Canary streaming documentation](https://docs.nvidia.com/nemo-framework/user-guide/latest/nemotoolkit/asr/streaming_decoding/canary_chunked_and_streaming_decoding.html).

## Recommended order and acceptance criteria

1. Try the existing Vulkan backend on the same Q8 model, then OpenVINO if needed.
   Require complete transcripts and paired warm measurements before integration.
2. Benchmark Moonshine streaming against both Canary and our existing streaming
   Parakeet option on public human speech, including accents, names, silence and
   long recordings. Measure both total compute and release-to-final-text delay.
3. For an earlier product improvement, process completed Canary chunks during
   recording and retain one recording/history entry. This reuses the existing
   chunking approach and can leave only the last chunk after release. It does
   little for short dictations and does not halve CPU work. Keep inference off
   the audio/UI thread and test cancellation and chunk boundaries.
4. Only then invest in CPU graph optimization or Token Map Drafting. Training
   Medusa heads or building a runtime from scratch has much higher maintenance
   cost and is not justified by the current evidence.

A backend experiment is cheap to reverse. Making a new model the default is a
more consequential choice because accuracy and language behavior change. Keep
the current default until a candidate achieves at least 2× paired warm speed
with an agreed accuracy tolerance, no dropped endings or empty speech results,
and acceptable memory and idle CPU use. None has cleared that bar yet.

The isolated scripts, instrumented driver and raw outputs are saved locally in
`artifacts/canary-speed-research/`. Scratch builds and alternative weights use
`/tmp/canary-speed-research/`; user model caches and configuration are unchanged.
The Q4 download uses the same Hugging Face revision as Q8,
`b147f9dc52b59f0998e410540a84727bd86457fd`, with filename
`canary-180m-flash-Q4_K_M.gguf`. Fixture generation and base dependencies are
documented in [README.md](README.md).

## Follow-up implementation experiments

We subsequently built and measured custom kernels and graph changes. Direct
depthwise convolution was 1.07–1.12× faster on the speech fixtures and reduced
peak memory by about 26%; no candidate reached 2×. See the
[kernel experiment results](kernel_experiment/RESULTS.md) for code, numerical
checks and the paired comparison.
