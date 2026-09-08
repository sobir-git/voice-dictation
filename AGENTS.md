# Voice Dictation

The UI is native Rust on Fire UI. The Rust speech daemon in `service/` owns
recording, global hotkeys, CTranslate2 inference, history writes and text output.
Keep those operations off the UI thread. The adapter is headless, not another UI.

Consume [Fire UI](https://github.com/sobir-git/fire-ui) through public APIs, pinned
to a published Git tag or commit in Cargo.toml. Keep framework code upstream.
Fire UI is also our project; propose
reusable framework improvements through GitHub issues or pull requests in that repo.
The previous GTK UI belongs only in Git history. No compatibility UI or aliases.

Keep the app dark, fast to resize, and quiet when idle. Preserve user configuration,
SQLite history and cached speech models. Config lives only in
`~/.config/speech-to-text/config.yaml`; the project config example is not active.
Use temporary data and synthetic dictations for screenshots and automated checks.

Run `cargo test --locked`, strict Clippy, Python unittest discovery, and the native
probe described in README.md. Inspect native screenshots. Keep docs short while
this design is evolving. The launcher activates the input group and opens the
native desktop; only the headless daemon autostarts.
