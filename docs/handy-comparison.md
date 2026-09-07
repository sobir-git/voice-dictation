# Handy reference review

Reference checkout: `/home/fire/projects/Handy`.
Repository: [cjpais/Handy](https://github.com/cjpais/Handy).
Inspected commit: `bc7facea3a777869182203cfcf5c90f7a98efd99`.

Handy uses a Rust/Tauri backend and a React interface. Its code is MIT licensed. This project uses independently written Python/GTK implementations inspired by the interaction and lifecycle patterns below; no Handy code or brand assets were copied.

## Applied ideas

| Handy pattern | Application here | Reference |
| --- | --- | --- |
| Recording readiness follows the first actual samples | Show "Opening microphone" until the recorder observes samples; log first-sample latency | [Audio manager](https://github.com/cjpais/Handy/blob/bc7facea3a777869182203cfcf5c90f7a98efd99/src-tauri/src/managers/audio.rs) |
| Recording overlay reacts to microphone levels | Feed the dark recording display actual level measurements; keep the dedicated microphone test | [Recording overlay](https://github.com/cjpais/Handy/blob/bc7facea3a777869182203cfcf5c90f7a98efd99/src/overlay/RecordingOverlay.tsx) |
| Cancellation invalidates stale asynchronous work | A cancellation generation suppresses late transcription delivery and discards queued recordings | [Audio manager](https://github.com/cjpais/Handy/blob/bc7facea3a777869182203cfcf5c90f7a98efd99/src-tauri/src/managers/audio.rs) |
| Explicit model status | Show the selected model and readiness in the window header and startup state | [Model status](https://github.com/cjpais/Handy/blob/bc7facea3a777869182203cfcf5c90f7a98efd99/src/components/model-selector/ModelStatusButton.tsx) |
| Live logs and adjustable log level | Diagnostics supports debug logging, live follow, runtime report, and copy | [Debug settings](https://github.com/cjpais/Handy/blob/bc7facea3a777869182203cfcf5c90f7a98efd99/src/components/settings/debug/DebugSettings.tsx) |

## Useful next work

- **Microphone stream reuse.** Handy offers on-demand and always-on modes, plus delayed stream closure. That could reduce the initial capture delay. It changes when the microphone remains open, so it should be an explicit user setting rather than a hidden optimization.
- **Model library and download progress.** Handy distinguishes download, verification, loading, and ready states. A fuller model page would make large-model startup less confusing. The present app shows loading and uses the local cache first.
- **Hold versus toggle shortcuts.** Handy offers both. Hold-to-record remains this app's current behavior. Toggle mode would help longer dictations but needs explicit stop/cancel handling and tests for accidental activation.
- **Clipboard paste with restoration.** This can deliver long or multilingual text faster than synthetic typing. Handy saves and restores clipboard contents and handles tool failure. Supporting rich clipboard contents correctly adds work; direct typing and explicit history copy remain the current paths.
- **Parakeet and streaming transcription.** Handy provides additional inference engines and streaming state. These are substantial engine changes, not drop-in speed flags for faster-whisper. They need a local latency/accuracy benchmark before adoption.

The native GTK revamp keeps this project's existing installation and transcription engine. A wholesale Tauri rewrite would add packaging and platform work without addressing the immediate silent-input and concurrency failures.
