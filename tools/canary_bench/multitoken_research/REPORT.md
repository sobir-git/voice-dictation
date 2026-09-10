# Multi-token prediction for Canary 180M Flash

Date: 2026-09-10

## Decision

Multi-token prediction is worth a small, instrumented feasibility experiment, but it is not a
candidate for the current default and it does not count toward the separate
base 2× target. The least risky first experiment is model-free Token Map
Drafting over a small, local dictation corpus. It needs no changed Canary
weights and can answer the key question, accepted tokens per verification,
before anyone trains heads or adds a second neural model.

Medusa-style heads are a second-stage research option only if the map experiment
shows that the 4-layer decoder has enough predictable runs to amortize a
verification pass. They require new trained parameters and a new parallel
decoder graph. A combined 4× result remains an aspiration. It must be measured
as one full pipeline, never obtained by multiplying paper results by the base
benchmark.

## What the methods actually are

These methods are often called multi-token prediction even though they have
different correctness and training requirements.

| Method | Proposal source | Verification | Existing Canary weights | Main risk |
| --- | --- | --- | --- | --- |
| Token Map Drafting | n-gram token map built from domain text | full model checks a proposed prefix | retained unchanged | map coverage and low acceptance on names, errors, and language changes |
| Draft-model speculation | smaller ASR/LM | full model checks draft tokens | retained, plus a second model | draft model cost and two model memory |
| Medusa / trained heads | extra heads on target hidden state | target decoder verifies candidates in parallel | new head weights required | training mismatch, graph complexity, rejected suffix handling |
| Unverified parallel decode | model predicts several positions and accepts all | none | changed model/training | changes transcript quality and end-token behavior; not lossless |

Token Map Drafting reports 1.27× and 1.37× decoder speedups on structured
Whisper domains with unchanged recognition accuracy. Those are decoder results,
not complete ASR results, and Whisper is a different architecture. Its useful
property here is the lack of a second neural model. [Paper](https://arxiv.org/abs/2507.21522)

Whisper-Medusa adds multiple heads and reports about 50% lower latency. The
paper is ASR-specific, but it changes Whisper training and decoding. It does
not provide a drop-in head for Canary. [Paper](https://arxiv.org/abs/2409.15869)

Medusa's general method makes the distinction explicit: Medusa-1 trains heads
on top of a frozen backbone, while Medusa-2 fine-tunes the backbone and heads.
The paper reports over 2.2× for Medusa-1 and 2.3–3.6× for Medusa-2 on LLM
benchmarks. Those numbers do not transfer to Canary's small encoder-decoder or
to an Intel integrated GPU. [Paper](https://arxiv.org/abs/2401.10774)

SpecASR is closer in spirit to ASR deployment. It uses a draft ASR model,
adaptive draft length, draft recycling, and sparse token-tree verification. It
reports 3.04–3.79× over autoregressive decoding and 1.25–1.84× over ordinary
speculation in its LLM-based ASR experiments. The target setup uses a large
Vicuna-style decoder, so its acceptance and compute balance are not evidence
for Canary 180M. [Paper](https://arxiv.org/abs/2507.18181)

The original seq2seq speculative-decoding work establishes the basic exactness
contract: a cheap proposal is checked by the target, and only the accepted
prefix is committed. [Paper](https://arxiv.org/abs/2203.16487)

## Why Canary is a special port

The pinned local source describes Canary as a multitask AED model. Canary-180M
Flash has a FastConformer encoder and a four-layer autoregressive Transformer
decoder. The public port supports offline transcription and translation in
English, German, Spanish, and French, with an explicit multitask prompt.
[Porting notes](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/porting/families/canary.md),
[model notes](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/canary-180m-flash.md)

The local `arch/canary/decoder.h` exposes three relevant paths:

1. A prompt graph handles multiple prompt tokens with `n_tokens > 1`.
2. `build_step_graph` creates a static topology for one generated token. It
   writes self-attention K/V through a runtime index and reads a masked fixed
   window.
3. Cross-attention K/V is populated once from encoder output and then reused.

The shared `run_batched_encdec_step_loop` feeds one token per step, grows the
self-KV read window by rebuilding the graph, and stores generated token IDs.
That means a verifier needs a new multi-token graph with causal positions and
an output for every proposed position. It cannot simply call the current
one-token step repeatedly and claim verification parallelism.

The local `CanaryKvCache` has separate self and cross K/V tensors. Cross K/V is
safe to keep across a verification round. Self K/V is the dangerous part. A
proposal pass may write K/V for tokens later rejected. The runtime must either
write those rows into a scratch suffix or restore the committed suffix from a
checkpoint. On rejection at position `p`, commit tokens before `p`, discard
all later proposal rows, then run the verifier-selected token at `p` and write
its K/V into the committed row. A token-only rollback is insufficient if the
cache rows remain contaminated.

## Training and model cost

Token Map Drafting keeps the downloaded Q8 GGUF unchanged. The map is a small
sidecar generated from representative transcripts. It is the only option that
can be evaluated before obtaining a NeMo training environment.

Medusa heads need hidden-state-to-vocabulary projections for future offsets.
For Canary, each head would consume a decoder hidden state and predict a token
at an offset beyond the current position. The heads need training on the same
multitask tokenization and prompt format, with loss on shifted future tokens.
The model's untied vocabulary head is already a large matrix. A naive K-head
design adds K vocabulary projections, which can be large relative to the 182M
parameter model and expensive to run on CPU. A shared low-rank head may reduce
memory, but changes the acceptance tradeoff and needs its own training test.

A draft-model route needs a second tokenizer-compatible decoder. A tiny CTC
encoder draft is not automatically valid for this AED decoder because its
timing and token conditioning differ. It also adds model loading and transfer
cost. On an Iris Xe iGPU, two small kernels and host/device synchronization can
erase the saved target steps.

No existing Canary checkpoint contains trained multi-token heads. Adding metadata to a GGUF
without trained tensors would only make the loader accept a file; it would not
make useful predictions.

## Intel Iris Xe implications

The parent benchmark reports 4.1 ms per decoder token on Intel Iris Xe ADL GT2,
with matrix operations already fused with bias and residual paths. Encoder and
decoder are both substantial, with warm short timings around 187 ms encoder and
399 ms decoder, and medium timings around 515 ms encoder and 1071 ms decoder.

Multi-token prediction can only reduce decoder work. The encoder remains. If a
verified decoder method halves decoder time, a simple stage model predicts a
full-pipeline gain closer to 1.4× on the medium split, before proposal and
verification overhead. This is a bound from the supplied timings, not a
measurement.

Verification needs a batch of candidate positions. That may improve matrix
utilization, but the target's Q8 weights are then reread for a larger matrix
operation and the graph needs more activation memory. Packed4x8 integer dot
hardware helps the existing matrix kernels, while the lack of cooperative
matrix support makes a large new tree or batched head less attractive. Small
candidate batches also pay command submission and synchronization overhead.

The relevant measurement is therefore accepted tokens per target verification,
proposal time, verification time, cache-copy time, and complete encoder plus
decoder wall time. Tokens per second from the decoder alone is insufficient.

## Correctness and end-token hazards

Greedy Canary decoding has a clear exactness target: for accepted prefixes, the
transcript must match the baseline token-for-token. The experiment must treat
EOS as a normal verified token. A proposal that includes EOS must not cause the
driver to stop until the target verifier selects EOS at that position.

The test corpus must include:

- short clips where the transcript ends near the first proposed block;
- silence and near-silence, which should remain empty;
- names, numbers, punctuation, and rare words;
- long recordings and chunk boundaries;
- the 73 public human clips from the supplied one-speaker manifest;
- synthetic fixtures, so failures are reproducible without changing user data.

Record exact token agreement, WER or word errors, missing endings, premature
EOS, extra trailing text, accepted-prefix length, and abort behavior. A speed
win that drops the final reminder or truncates a chunk is a failed candidate.

## Host-only token-map experiment

I ran the bounded simulator in `token_map_sim.py` against the supplied
`/tmp/canary-vulkan-research/human/manifest.json` across all five deterministic
case-ID folds. Each fold builds its map from the other folds. All 73 clips are
speaker 1272 and excerpts from the same source book, so the split prevents
case-level training/evaluation reuse but does not remove speaker or shared-book
leakage. The map uses reference words as a proxy for Canary tokens because this
experiment did not run inference or parse the GGUF tokenizer.

| Context words | Proposed words | Total tokens | Total rounds | Coverage | Accepted/round | Effective tokens/round |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 1 | 1223 | 1181 | 0.528 | 0.036 | 1.036 |
| 1 | 2 | 1223 | 1190 | 0.532 | 0.028 | 1.028 |
| 1 | 4 | 1223 | 1187 | 0.531 | 0.030 | 1.030 |
| 2 | 1 | 1223 | 1211 | 0.078 | 0.010 | 1.010 |
| 2 | 2 | 1223 | 1209 | 0.077 | 0.012 | 1.012 |
| 2 | 4 | 1223 | 1209 | 0.077 | 0.012 | 1.012 |
| 3 | 1 | 1223 | 1221 | 0.008 | 0.002 | 1.002 |
| 3 | 2 | 1223 | 1221 | 0.008 | 0.002 | 1.002 |
| 3 | 4 | 1223 | 1221 | 0.008 | 0.002 | 1.002 |

The raw aggregate is in [evidence/aggregate.json](evidence/aggregate.json).
Bootstrap intervals from the earlier fold-0 run were clip-level diagnostics,
not population intervals. The fold runner also writes one small JSON result per
fold under `/tmp/canary-multitoken-luna/evidence/`.

The result is weak. A one-word context has high lookup coverage because the
corpus repeats common function words, but the most common continuation is
usually wrong, so the accepted prefix stays near zero. Longer contexts sharply
reduce coverage. Increasing K does not help this deterministic top-1 map and
slightly reduces effective progress when a later proposed word mismatches.
The measured Python evaluation cost was about 8–14 microseconds per clip, but
that is only host lookup overhead and says nothing about Canary graph cost.

The simulator accounts for bonus words and EOS explicitly. On mismatch, the
verifier commits the accepted prefix plus one target-selected token. A proposed
EOS before the target EOS is rejected. When the target EOS is reached, it is
committed even if the map has no proposal. A map with zero coverage produces
exactly one effective token per round, so it cannot report a speedup by skipping
work. Tests cover mismatch-first, partial acceptance, all acceptance with a
remaining target, premature EOS, final EOS on a miss, and zero-coverage
accounting. See [TESTING.md](TESTING.md).

This proxy cannot establish model-token acceptance, WER, KV-cache correctness,
or inference speed. Reference text is correlated with the map's training data
distribution and is easier to predict than model errors. The result therefore
does not justify implementing verification or training heads. It only gives a
lower-cost screen for reference-text continuation predictability.

## Concrete next experiment

Build a local harness under the research directory with a map-only proposal
layer. Do not alter the live runtime yet.

1. Use the existing baseline executable and the supplied Q8 GGUF. Run the
   synthetic f32 fixtures and the 73 human clips once with deterministic greedy
   decoding. Save token IDs, text, per-stage timings, and EOS position.
2. Create a token map from only the training side of the provided manifest, or
   a clearly separated subset. Store top candidate continuations keyed by the
   last 2–4 token IDs. Do not build the map from evaluation transcripts.
3. Add a pure host-side simulator that proposes up to K=2 tokens, then compares
   each proposal with baseline target token IDs at that position. This first
   pass estimates acceptance without touching KV state. Measure K=1, 2, 4,
   accepted-prefix length, map lookup time, and coverage.
4. If the simulator reaches at least 1.5 accepted tokens per proposal round on
   human speech and map lookup is below 1% of decoder time, implement a scratch
   verifier graph. Use a separate suffix buffer for self-KV and commit only the
   accepted prefix. Keep cross-KV shared.
5. Compare baseline and verified runs token-for-token before measuring speed.
   Then measure warm full-pipeline median, encoder, decoder, proposal,
   verification, cache management, RSS, and idle CPU. Run each candidate in a
   separate process to avoid parent benchmark interference.

The first go/no-go threshold should be exact transcripts on all synthetic
fixtures, zero missing endings or premature EOS on the human set, and a
measured full-pipeline improvement after all overhead. A useful second gate is
at least 1.25× decoder speedup and at least 1.10× complete inference speedup.
Those are experiment gates, not promises.

If Token Map Drafting fails acceptance or does not beat overhead, stop that
proposal path. A trained Canary Medusa head remains a separate model-training
experiment with a higher reversal cost. Do not gate that experiment on an
n-gram map result, since the proposal mechanisms have different failure modes.

## Recommendation to the parent track

Keep multi-token prediction marked as extra acceleration. Continue reporting the base kernel and
GPU work independently. If the map simulator clears the acceptance gate, the
parent can decide whether the added runtime complexity is worth a small
decoder-only gain. If it fails, the evidence is still useful: it rules out a
cheap exact acceleration path for this Canary workload without changing model
weights or app behavior.
