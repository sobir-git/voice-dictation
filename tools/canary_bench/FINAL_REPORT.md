# Inference research: final bounded result

**The unchanged Q8 model reached 1.97× on a fixed short-clip set and 1.76×
on complete short utterances. A reliable general 2× improvement was not
established.** No experimental backend was installed into the app.

Intel i5-12500H / Iris Xe, Linux. Each comparison used three process cycles,
rotating variant order, with two rounds per process. The table sums each
clip's median warm latency; setup is excluded. Every clip is at most five
seconds. These are workstation measurements, not a controlled performance lab.

| Comparison | Default CPU | Candidate | Speedup |
| --- | ---: | ---: | ---: |
| Nine mostly five-second clips: CPU tuning only | 2.324 s | 1.609 s | 1.44× |
| Same nine clips: GPU encoder + CPU decoder | 2.324 s | 1.222 s | 1.90× |
| Same, GPU FP32 arithmetic enabled | 2.324 s | 1.179 s | **1.97×** |
| 31 complete utterances, 1–5 s, final candidate | 5.883 s | 3.335 s | **1.76×** |

The candidate keeps the original Q8 weights. It uses direct frontend depthwise
convolution on the GPU, moves decoding to the CPU, and pins eight CPU threads
to the four performance cores. Disabling GPU FP16 arithmetic helped slightly
on this Iris Xe. CPU-only tuning uses four performance-core threads.

Active OpenMP waiting helped latency but initially consumed about three cores
at idle: 6.00 CPU-seconds during a two-second idle window. Explicitly pausing
the workers after every dictation reduced measured idle use below 0.0001
CPU-seconds per two seconds. Pause overhead is included in candidate timing.
Never adopt active waiting alone.

All paired warm transcripts matched. On the 31 complete utterances, both
baseline and candidate made 9 word errors in 256 reference words (3.52%, using
simple lowercase word normalization). **All human speech comes from one
speaker.** This does not establish accuracy across accents, languages, noise,
long recordings or silence. The cropped set is a latency/stability check,
not a reference-WER test.

Median model loading was 144 ms for the baseline and 171 ms for the final
candidate. Peak process RSS was 434 and 227 MiB respectively, but GPU allocations
are not fully represented by process RSS; this is not a total-memory saving
claim. The prototype retains duplicate weight storage and recreates schedulers.
It needs ownership, memory, cancellation and varied-input validation before
production use.

Other results:

- Handy's ONNX path and the corrected OpenVINO CPU screen were slower. The
  bounded OpenVINO GPU attempt did not produce a completed warm timing.
- GPU matrix tile changes and CPU weight packing won some isolated kernel
  tests but did not reliably improve complete transcription. They are excluded
  from the final candidate.
- Q4 changed some transcripts and did not reliably deliver 2× overall. Its tiny
  111-word quality screen is insufficient to accept a precision change.
- Luna's multi-token word-map simulation achieved only 1.036 word/EOS units per
  verification round. It is not a Canary inference speedup. Trained heads remain
  a separate backlog item and contributed nothing to the results above.

Iteration improved substantially: editing two GPU shaders went from roughly
60 seconds of native rebuilding to 0.22 seconds of standalone compilation.
Small matrix probes take about 0.2–0.3 seconds; routine transcription uses one
five-second clip. Native C++ changes still require a rebuild.

The experiment was stopped within the user's additional one-hour cap. No
Handy issue was posted: Handy already has a transcribe.cpp custom-GGUF path,
and the experiments did not establish the originally suspected ONNX advantage.

[Reproduction and implementation notes](intel_gpu_experiment/README.md) ·
[Fixed-clip evidence](intel_gpu_experiment/evidence/final-validation/summary.json) ·
[Complete-utterance evidence](intel_gpu_experiment/evidence/complete-short-validation/summary.json) ·
[Multi-token report](multitoken_research/REPORT.md)

Follow-up: [duration and RAM comparison](../model_speed_experiments/DURATION_REPORT.md) measures 2, 5, 15, 30 and 60 seconds and includes integrated-GPU resident buffers separately. Use that report for memory comparisons; process RSS alone omits substantial system RAM held by the GPU.
