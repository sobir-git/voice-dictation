# Canary multi-token research

This directory contains the multi-token prediction research track. It is
separate from the base inference target in `tools/canary_bench/SPEED_BACKLOG.md`.
No live app code, model cache, user configuration, or shared benchmark scratch
directory is part of this track.

- [Feasibility report](REPORT.md)
- [Source register](SOURCES.md)

The report uses the pinned `transcribe-cpp-sys-0.2.3` source as the local
runtime reference and treats paper speedups as evidence about a method, not as
Canary measurements.
