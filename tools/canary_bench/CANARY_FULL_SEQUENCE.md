# Canary full-sequence Vulkan screen

This is a new opt-in experiment. It does not replace the historical Canary
measurements in `FINAL_REPORT.md` or `SPEED_RESEARCH.md`.

## Hypothesis

Canary is non-streaming in the app, but ordinary inference splits recordings
longer than 30 seconds at quiet boundaries. Sending one accepted recording to
one `session.run` call may reduce repeated setup and decoder work.

## Method

- Host: 12th Gen Intel Core i5-12500H with Intel Iris Xe Graphics, Mesa Vulkan.
- Model: `canary-180m-flash` Q8 GGUF from the existing Hugging Face cache.
- Build: release binary with the `vulkan` feature.
- Settings: English, int8, beam size 5, VAD enabled, 8 threads.
- Each case used a fresh process, one warmup and two timed runs.
- The daemon was stopped during these direct cases so both profiles had the GPU
  to themselves.
- The active user configuration was not used or changed by the cases.

| Audio | Ordinary `vulkan` | `vulkan-full` | Speedup | Stable words |
|---:|---:|---:|---:|---|
| 5s | 1.185s | 0.617s | 1.92x | yes |
| 15s | 5.123s | 1.717s | 2.98x | yes |
| 30s | 7.174s | 3.218s | 2.23x | yes |

The 5-second, 15-second and 30-second transcripts matched between the two
profiles, and each profile was stable across its repeated runs. These outputs
are not a ground-truth accuracy evaluation.

The full path rejects Canary recordings of 40 seconds or longer. A direct
60-second case returned the expected error before inference:
`Experimental Canary vulkan-full requires audio shorter than 40 seconds`.

## Decision

Keep the path experimental and opt-in. The speed gain is real for this
completed-file screen, but Canary has no live preview to disable and the short
inputs do not remove chunking work. Test recordings above 30 seconds, human
speech and stop-to-final-text behavior before considering a broader rollout.
