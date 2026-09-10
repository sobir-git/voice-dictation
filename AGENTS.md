# Voice Dictation

This is a native Rust desktop built on Fire UI. The Rust daemon in `service/`
owns recording, global hotkeys, speech inference, history and text output. Keep
that work off the UI thread. The adapter is headless.

Parakeet Unified streams through transcribe.cpp by default. Whisper through
CTranslate2 remains the fallback. Preserve user configuration, SQLite history
and cached models. The active config exists only at
`~/.config/speech-to-text/config.yaml`; `config.yaml.example` is a template.

Use Fire UI through public APIs pinned to a published tag or commit. Keep reusable
framework changes upstream in `sobir-git/fire-ui`. Do not restore the old GTK UI
or compatibility aliases.

Keep the app dark, responsive when resized and quiet when idle. Use temporary
data and synthetic dictations in automated checks. Only the daemon autostarts.

For an integration or release handoff, run the full gate:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s tests -v
cargo build --release --locked
python3 tools/native_probe.py
```

Inspect the native screenshots. Use `./update_local.sh` to install and restart the
local development build without replacing user data.

For ordinary task handoffs, use the smallest verification tier that covers the
changed behavior. Run `python3 tools/verify.py --help` for the executable runner.

- `ui-layout`: UI tests and lint, one CPU release artifact, and one native probe.
- `ui-behavior`: UI tests, lint, Python service tests, one CPU release artifact,
  and one native probe.
- `full`: the complete gate above, including service tests and the native probe.

Use `--backend vulkan` only when the change affects the Vulkan build or its
runtime. The runner defaults to CPU, never installs or restarts the user's
application, and reuses a release artifact only when its source, toolchain,
feature set, build flags and target-specific provenance still match. Use
`--no-reuse` to force a fresh artifact or `--dry-run` to inspect the selected
commands. Unknown or cross-cutting changes require `full`.

For terminal-only optimization discovery, benchmark/apply commands, JSON contracts,
and host-specific build composition, see [docs/agent-performance.md](docs/agent-performance.md).
Do not add a benchmark or installer UI; keep optional acceleration dependencies out
of the default CPU installation.

For continuing optimization work, start with
[docs/optimization-development.md](docs/optimization-development.md). It maps code
extension points, the measurement routine, existing evidence and the research backlog.
