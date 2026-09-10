# Inference optimization backlog

## Current target: 2× without multi-token prediction

User priority, 2026-09-10: pursue a genuine 2× improvement in complete inference,
including architecture-specific kernels, weight packing, execution changes and
Intel GPU acceleration. Building a specialized inference engine is in scope if
the evidence justifies it. The later one-hour cap limits this research session.

Keep the same model/task and evaluate transcription quality. Report cold setup,
warm inference, memory and correctness separately. Multi-token prediction does
not count toward this first 2× target.

## Extra acceleration: multi-token prediction

Explicitly requested by the user as a separate research track after the base
inference work. The aspiration is an additional boost toward 4× overall; that
is a target, not a measured or guaranteed multiplier.

Research tasks:

- Distinguish trained multi-token prediction heads from draft-model speculative
  decoding, token-map drafting and parallel decoding without verification.
- Find current ASR-specific implementations and evaluate transfer to Canary's
  encoder-decoder architecture. Do not assume an LLM result transfers directly.
- Determine whether existing weights can be retained or new heads/fine-tuning
  are necessary. Account for training data, training cost and deployment memory.
- Design candidate verification, causal masking and KV-cache rollback so rejected
  predictions cannot corrupt subsequent transcription.
- Measure accepted tokens per verification, draft/verification overhead and the
  remaining encoder cost on the already optimized implementation.
- Evaluate public human speech, long recordings, silence, names and supported
  languages. Check missing endings and premature end-of-sequence explicitly.

Acceptance: report the base implementation's independently measured speedup and
the additional multi-token speedup separately. Measure the combined full-pipeline
result directly rather than multiplying unrelated paper or microbenchmark gains.

Status: initial research and a host-only word-map proxy completed by the Luna
subagent at the user's request. See [report](multitoken_research/REPORT.md).
Across five held-out folds, the best proxy achieved 1.036 word/EOS units per
simulated verification round. This is not a measured Canary speedup and does
not rule out trained multi-token heads.

## Session closed within the one-hour cap

See [final measurements](FINAL_REPORT.md): unchanged Q8 reached 1.97× on a fixed
short set and 1.76× on 31 complete short utterances. A reliable general 2× was
not established. The prototype remains isolated; no backend replacement was
installed. Further experiments require a new work session.

Potential follow-up: production ownership and memory accounting for the hybrid
path; worker pause/cancellation integration; more speakers and noisy/long audio;
shape-dependent GPU dispatch overhead. Preserve multi-token prediction as an
independent future track, with trained heads still untested.
