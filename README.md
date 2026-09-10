# Voice Dictation

Local Linux voice dictation with a native [Fire UI](https://github.com/sobir-git/fire-ui)
desktop and a Rust speech service. Hold your hotkey, speak, and release to type.
Parakeet Unified is the default English model and streams through transcribe.cpp
while the hotkey is held. Whisper remains available through CTranslate2, with
Silero voice activity detection. Python is used only for development probes.
Canary 180M Flash is also available through transcribe.cpp for English, German,
Spanish and French; it runs offline after downloading its 208 MB Q8 model.

```sh
./install.sh
./setup_autostart.sh
./run.sh
```

The desktop uses Fire UI’s Ember Dark theme, native tabs, dropdowns and switches.
Framework layout and scrolling handle resizing; Cairo draws the window.
It includes microphone levels, an editable transcript, searchable history,
settings and diagnostics. Closing it leaves dictation running. The tray reopens the
desktop, pauses dictation and cancels pending work. Quit Voice Dictation closes the
app and stops the service; opening the app starts it again. Enable Floating indicator in
Settings for a passive recording/transcribing widget near the pointer. It currently
uses X11/XWayland and does not take keyboard focus or intercept clicks.

## Requirements

Rust, CMake, a C++ compiler, `alsa-utils`, `ffmpeg`, and
native X11, Cairo and FreeType libraries, including `libxkbcommon-x11-0` on Debian/Ubuntu. Building also needs
`pkg-config`, `libcairo2-dev` and `libfreetype6-dev`.
A system font is required; `FIRE_UI_FONT` can select a font file.
The first build compiles CTranslate2 and oneDNN;
ONNX Runtime is downloaded for Silero. Inference defaults to CPU, with opt-in Vulkan for GGUF models.

Global hotkeys require the `input` group. The launcher activates it when needed.
GNOME Wayland normally uses `ydotoold`; X11 normally uses `xdotool`. PipeWire ALSA
support and `pactl` enable microphone selection. Only the headless service autostarts.

Configuration stays in `~/.config/speech-to-text/config.yaml`; the project example
is not active. SQLite history stays in `~/.local/share/speech-to-text/history.db`.
Existing Parakeet, Canary and faster-whisper models are reused from the Hugging Face cache.
Installation preserves configuration, history and models. Uncached models download
on first use. Streaming previews stay in the native desktop; only finalized text is
written to history and the focused application.

```sh
./target/release/speech-service --transcribe recording.wav
journalctl --user -u speech-to-text-daemon.service
```

File transcription prints JSON without typing text or adding a history entry.
Output failures during dictation leave text in History. Cancellation suppresses
pending output but cannot retract text already typed.

## Development

`ui/` uses the GitHub Fire UI dependency pinned in Cargo.toml. `service/` owns the
daemon, socket protocol, headless adapter and tray. No speech inference runs on the
UI thread. Microphone tests keep samples in memory and never transcribe them.

Fire UI is also our project. Consume its public APIs here and propose reusable
framework improvements through [upstream issues or PRs](https://github.com/sobir-git/fire-ui).
The app pins the published [Fire UI v0.8.0 release](https://github.com/sobir-git/fire-ui/releases/tag/v0.8.0).

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 -m unittest discover -s tests -v
cargo build --release --locked
python3 tools/native_probe.py
dbus-run-session -- ./venv/bin/python3 tools/tray_probe.py
python3 tools/service_ui_probe.py
```

Install the current working tree and restart the local service with one command:

```sh
./update_local.sh
```

The script reopens the desktop only when it was open before the update. It preserves
the existing configuration, history database and cached models.

The probes use temporary data and synthetic dictations. `tools/migration_probe.py`
optionally compares Rust against faster-whisper using synthesized speech. The native
probe requires Xvfb, xdotool, xclip and Pillow. The tray probe requires dbus-next.
Install probe dependencies from `requirements-dev.txt`.
Review diagnostics before sharing; they include local device names and paths.

## PhoneMic recording control

The sibling PhoneMic app can start and stop dictation from its phone page.
Enable **Trigger laptop dictation** there and keep this daemon listening. Mobile
recordings use the phone microphone without changing the saved desktop input.
Text uses the normal history and output flow after release.

Local socket clients can send `start_recording` with a `pipewire_node`, then
`stop_recording` to submit or `abort_recording` to discard. Wait for
`recording_started` and an `audio_level` event before sending microphone audio.
Only the initiating connection can stop or abort that recording; disconnecting
it discards the capture. Hotkey release does not stop a client-owned recording.
The socket remains private to the local user; no HTTP control endpoint is added.

## Performance experiments

The service includes a terminal-only benchmark runner and selectable inference profiles. Benchmarking does not change settings. Applying a result saves it per model, and switching models restores that model's backend automatically. See [the guide](docs/performance-lab.md) for commands, RAM comparisons and experimental short-context inference. The default build stays CPU-only. For GPU support, build with `--features vulkan` or install with `VOICE_DICTATION_FEATURES=vulkan ./install.sh`; builders need Vulkan development headers, loader and a recent glslc compiler. The SDK is not shipped with the app.

Settings shows the active inference engine and CPU or Vulkan backend, including CPU fallback. History shows each attempt's model and end-to-end transcription time, including queueing and model loading after recording stops or a retry is requested. Audio duration remains separate. Existing rows without recorded metadata show unknown values.
