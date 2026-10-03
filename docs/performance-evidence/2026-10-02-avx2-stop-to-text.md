# Fixed AVX2: actual post-recording wait

On the same Intel i5-12500H laptop, compare the original conservative native
build with the fixed AVX2 preset, retaining the Vulkan edition and unchanged
Parakeet Q8 model, English, beam 5, VAD, int8 setting and automatic threads.
The native GPU log confirms Vulkan0 is physical Intel Iris Xe (Mesa).

| Backend / speech | Original median (s) | AVX2 median (s) | Samples per build | Original / AVX2 |
|---|---:|---:|---:|---:|
| CPU / 5 s | 5.526858887 | 1.271832041 | 3 | 4.35× |
| CPU / 15 s | 24.777552514 | 1.397374592 | 3 | 17.73× |
| Vulkan / 5 s | 1.354224190 | 1.255604493 | 5 | 1.079× |
| Vulkan / 15 s | 0.891155980 | 0.889487467 | 5 | 1.002× |

The metric is the monotonic IPC recording-stop request to the first painted
glyph in the destination application. Synthetic speech arrives at real-time
speed through private native PipeWire `pw-loopback` / `pw-play`, real `arecord`,
the actual daemon capture/stream/finalization/history/output path, and real
`xdotool` typing into a GTK destination. `XGetImage` observes glyph pixels;
each observation has a recorded lower/upper bracket. The reported first changed
poll is an upper bound. Isolated Xvfb excludes physical key dispatch and physical
compositor/panel refresh. These are warm-session wait ratios over this precise
same-host reference, not buffered throughput or kernel increments.

One actual cold 5 s dictation warms each isolated daemon and is excluded from
primary results. CPU has three warm samples per length/build. Vulkan has five
across the initial and reversed GPU session orders; the direction changes by
order, so there is **no clear Vulkan improvement**. Shared desktop workload,
powersave governor and thermal scheduling are visible limitations. Do not claim
p95 or significance with these sample counts. The first long dictation follows
a short warmup and can include first long-shape effects. Actual capture WAVs
are approximately 5.37–5.50 s and 15.36–15.49 s, including transport silence.

All 15 words match in the short case. The long case has a stable 46-word prefix;
the clipped final word varies between `the` and `them` once in each CPU build.
This is a narrow synthetic regression check, not broad accuracy validation.
Worker lifetime VmHWM is 1186.863 MiB original CPU and 1188.488 MiB AVX2 CPU:
it includes loading/warmup/prior dictations and excludes daemon/target/GPU
buffers. It is not a per-case peak, and the strict zero-RSS-increase criterion
is not established. GPU buffer memory is not measured here.

The unchanged model is 731357568 bytes, SHA-256
`4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795`.
The fixture is the existing 60 s `assets/benchmarks/speech.flac`.
The laptop benchmark archive contains 32 warm primary samples, control scripts,
raw logs, representative synthetic capture WAVs and glyph screenshots; it
contains no model or user dataset. Archive SHA-256:
`b7e120342fc3a858b99e96d4174763d6b6afe360938749e8ba9a074217f99017`.
Raw VM evidence is under
`/workspace/remote-dev/evidence/voice-dictation/avx2-laptop-20261002/voice-stop-to-text-20261002T191157Z/`.
The original daemon, configuration, history and default audio devices were
restored unchanged after every group. The measured prototype was not activated
by that benchmark campaign; installation requires explicit owner authorization
and a matching passed artifact.

The shipping preset changes native compiler ISA only. No custom kernels, packed
weight caches, reduced precision, changed streaming context, new framework or
runtime-profile promotion are included. Further work should test other supported
CPUs and longer real utterances, and profile physical GPU kernels on the actual
GPU rather than extrapolate from software Vulkan.
