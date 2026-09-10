# Bounded OpenVINO screen

OpenVINO 2026.3.1 was tested with Handy's INT8 ONNX Canary model on an
Intel i5-12500H / Iris Xe. This experiment did not beat transcribe.cpp Q8.
No application backend was changed.

The five-second clip uses ONNX Runtime for Nemo preprocessing because the
STFT graph did not convert directly. OpenVINO runs the encoder and decoder.
The Flash prompt must contain `<|noitn|>`: an initial incorrect `<|itn|>`
prompt returned immediate EOS. Those empty-transcript timings are invalid
and excluded from the evidence here.

With the corrected prompt, CPU warm calls took 0.854 and 0.848 seconds,
including about 109 ms encoding and 735 ms decoding (43 tokens). The
transcribe.cpp Q8 CPU screen was about 0.288 seconds. The runtimes use
different quantized representations, so this is a backend alternative
comparison, not an identical-arithmetic kernel comparison.

The GPU was detected after extracting Intel OpenCL dependencies locally.
Initial model compilation took 15.35 seconds; cached compilation took
1.49 seconds. GPU transcription exceeded a 15-second process deadline;
encoding alone reported 525 ms. No completed warm GPU timing was obtained.
This rules it out for this bounded experiment, not for every OpenVINO
configuration. No system package or live app configuration was changed.

`screen.py` and `evidence/` retain the corrected CPU run and bounded GPU
attempt. Scratch dependencies live under `/tmp/canary-openvino-research`;
Python packages are in `/tmp/canary-vulkan-research/venv`.

[OpenVINO GPU configuration documentation](https://docs.openvino.ai/nightly/get-started/install-openvino/configurations/configurations-intel-gpu.html)
was used to configure the isolated GPU runtime.
