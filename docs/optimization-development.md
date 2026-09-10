# Continuing inference optimization

Start with [the agent command interface](agent-performance.md), then read the
[duration and memory report](../tools/model_speed_experiments/DURATION_REPORT.md).
Optimization is ongoing work. Keep each experiment small, reproducible and separate
from the default profile until its benefit and recognition quality are established.
The target remains 2× full-pipeline speed without multi-token prediction. An extra
boost from multi-token prediction is a separate target, not an established result.

## Where to change things

| Area | Entry point | What belongs here |
|---|---|---|
| Inference | `service/engine.rs` | Engine loading, CPP streaming, Whisper features and inference calls |
| Profiles | `service/optimization.rs` | Validation, supported models, thread settings, context selection and engine identity |
| Measurements | `service/optimization.rs` | Atomic case, paired subprocess runner, memory samples, report schema and CLI discovery |
| Worker lifecycle | `service/daemon.rs` | Queueing, unload/reload, cancellation, fallback and actual runtime status |
| Defaults | `service/config.rs`, `config.yaml.example` | Backward-compatible settings; never edit the active user config as a build step |
| Optional dependencies | `Cargo.toml`, `install.sh` | Opt-in acceleration features; keep SDKs and research artifacts out of installation |
| Integration checks | `tools/optimization_probe.py` | Temporary daemon, synthetic audio, benchmark cancellation and explicit apply |
| User wait time | `service/history.rs`, `service/daemon.rs` | Persist the attempted model and stop/request-to-inference completion time |

Host winners live under `performance.by_model`, keyed by exact model ID. The
top-level profile and thread count are the active model's resolved setting.
`--apply-optimization` updates both. Model switches preserve the outgoing choice
and activate the incoming choice, or Standard CPU if that model has no result.
Keep this behavior in the daemon so desktop and terminal clients agree.

To add a profile, update `validate`, `check_profile`, CLI capabilities and engine
dispatch together. Add every load-affecting setting to `identity`, or a cached
engine can silently retain an earlier configuration. Expose restrictions and
experimental status through JSON discovery. Keep a standard reference path.
Test the affected lifecycle when a change alters loading, queues or cancellation.

Native kernel changes belong in a reproducible upstream patch or pinned dependency
revision. Do not rely on edits inside a Cargo cache or a developer's `/tmp` tree.
The existing scratch drivers have host-specific paths. Their READMEs explain how
to reconstruct them; they are evidence and starting points, not portable tooling.

## Routine for each experiment

1. Record the source revision and local diff, dependency versions, build flags,
   model revision or file hash, host CPU/GPU, driver, precision, beam size, VAD,
   language and thread settings. Save the current profile for restoration.
2. Use a five-second clip to check the hypothesis. Change one thing at a time.
   Avoid inference and compilation running together. Separate model loading,
   warm inference and the end-to-end wait recorded in history.
3. Use the paired CLI for ordinary profile tests. Use `--benchmark-case` for a
   custom experiment, explicitly managing competing model processes yourself.
   Keep raw JSON, transcripts and the exact command in a new evidence directory.
   Never overwrite the existing reports to store a new run.
4. If the result looks useful, compare roughly 2, 5, 15, 30 and 60 seconds. Repeat
   close results and vary run order to detect thermal or scheduling bias. The
   built-in two measured repetitions are a screening tool, not statistical proof.
5. Report process peak RSS and GPU residency separately, including measurement
   method and missing counters. On integrated GPUs these can share system RAM;
   do not blindly add them or call RSS the entire application's memory use.
6. Check public or consented human speech, multiple speakers, noise, silence,
   names and supported languages. Use reference transcripts to measure errors.
   Matching baseline words on the bundled synthetic clip does not prove accuracy.
   Inspect missing endings, hallucinations and premature EOS explicitly.
7. Before adoption, check cancellation, model switching, history retry, CPU
   fallback, unsupported hosts and installation size. Run the required checks
   in `AGENTS.md`. Verify the actual runtime profile after applying settings.

For each result, retain a short note with: hypothesis, patch or revision, exact
invocation, fixture provenance, host, per-length times, memory, recognition
changes, limitations and a decision to adopt, reject or investigate further.
Report full-pipeline speedup directly. A kernel speedup is not an application
speedup, and multipliers from separate experiments must not be multiplied.

## Current state and next experiments

| Track | Established starting point | Next useful experiment |
|---|---|---|
| Parakeet | Existing transcribe.cpp Vulkan path is available as an optional app profile; earlier host measurements improved across tested lengths | Repeat on other Intel generations and discrete GPUs; investigate dispatch overhead, streaming shapes and thread counts |
| Whisper | `fast` skips zero-padding FFT work with bit-identical feature tests; `adaptive` is experimental and restricted to base.en | Broaden recognition tests around the five-second post-VAD boundary, then test context selection per supported model; reject short-context hallucinations |
| Canary | The integrated `hybrid` profile runs the encoder on Vulkan and decoder on CPU using a pinned transcribe.cpp fork | Reduce scheduler recreation and duplicated decoder storage; repeat duration and memory checks on other Intel GPU generations |
| CPU kernels | Same-model CPU path is the reference | Profile operator time, weight packing and shape-specific matrix kernels; evaluate ISA dispatch without assuming every host has the same instructions |
| GPU kernels | Existing Vulkan backend supplies a working baseline | Profile attention, fusion and dispatch costs before implementing tiled or fused attention; validate masking and numerical behavior against the reference |
| Multi-token prediction | Initial research and a host-only proxy exist; no measured ASR acceleration | Investigate actual model drafting or trained heads, verification and cache rollback, then measure acceptance and complete inference separately from the base 2× goal |

See the [Canary final report](../tools/canary_bench/FINAL_REPORT.md),
[kernel and GPU research](../tools/canary_bench/SPEED_RESEARCH.md),
[experiment backlog](../tools/canary_bench/SPEED_BACKLOG.md), and
[multi-token report](../tools/canary_bench/multitoken_research/REPORT.md).
Those reports describe completed research sessions. Their historical session
limits do not prevent a newly authorized experiment.

Keep benchmark orchestration terminal-only. Users or their agents choose profiles
for their own hosts. Avoid automatic profile promotion, duplicate resident models,
hard-coded CPU affinity and dependencies needed only by an experiment. If a
specialized inference engine becomes justified, prove it as an isolated backend
against the same measurements before replacing a working engine.
