# Voice Dictation

The UI is native Rust on Fire UI. The Python speech daemon owns recording, global
hotkeys, transcription, history writes and text output. Keep those operations off
the UI thread. `desktop.py` is a headless adapter, not another UI implementation.

Use the published Fire UI tag in Cargo.toml. Close reusable framework gaps in
`../fire-ui` through public capabilities; do not copy a framework into this app.
The previous GTK UI belongs only in Git history. No compatibility UI or aliases.

Keep the app dark, fast to resize, and quiet when idle. Preserve user configuration,
SQLite history and cached speech models. Config lives only in
`~/.config/speech-to-text/config.yaml`; the project config example is not active.
Use temporary data and synthetic dictations for screenshots and automated checks.

Run `cargo test --locked`, strict Clippy, Python unittest discovery, and the native
probe described in README.md. Inspect native screenshots. Keep docs short while
this design is evolving. The launcher activates the input group and opens the
native desktop; only the headless daemon autostarts.
