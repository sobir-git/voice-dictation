# Voice Dictation

Local voice dictation for Linux with a native [Fire UI](https://github.com/sobir-git/fire-ui)
desktop. Hold your hotkey, speak, and release to type into the active application.

```sh
./run.sh
```

The dark desktop includes dictation status and live microphone levels, an editable
transcript for copying, searchable history, microphone/hotkey/model settings, and
diagnostics. Closing waits for an in-flight settings save and leaves the speech service running. Reopen it from
the application launcher. The old GTK interface and tray have been removed.

## Setup

Requires Rust 1.88+, Python 3, a native linker, `alsa-utils` and the Python packages
in `requirements.txt`. Linux native graphics needs X11/Wayland and OpenGL libraries;
`ffmpeg`, `pactl`, and PipeWire ALSA support enable audio preprocessing and mic selection.
No GTK or AppIndicator dependency is required.

```sh
./install.sh
./setup_autostart.sh
```

Global hotkeys require the `input` group. The launcher activates it when needed.
GNOME Wayland uses `ydotoold` for typing; X11 normally uses `xdotool`.
Configuration remains in `~/.config/speech-to-text/config.yaml` and dictation history
in `~/.local/share/speech-to-text/history.db`. Installation preserves both.

## Development

The Rust client in `ui/` uses Fire UI's published `v0.1.0` Git tag. The Python adapter
in `src/speech_to_text/desktop.py` connects it to the existing headless daemon over
its private Unix socket. The daemon still owns audio capture, hotkeys, Whisper,
ordered transcription, history writes and text output. No speech inference runs
on the UI thread. Microphone tests keep samples in memory and never transcribe them.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
PYTHONPATH=src ./venv/bin/python3 -m unittest discover -s tests -v
cargo build --release --locked
python3 tools/native_probe.py
```

The native probe uses Xvfb, xdotool and Pillow with synthetic dictations. It does
not open a microphone or change live settings. `--demo` runs the same preview manually.
Settings in a smaller window scroll with the wheel. The diagnostics report contains
local device names and paths; review it before sharing.

Whisper downloads an uncached model on first use. Cached models load locally.
Output failures leave the text recoverable in History; cancellation cannot retract
text already typed. `journalctl --user -u speech-to-text-daemon.service` shows service logs.
