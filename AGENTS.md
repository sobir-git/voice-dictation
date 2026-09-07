# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Running the app

```bash
# Normal launch (ensures input group is active)
./run.sh

# Direct launch
./venv/bin/python3 stt_tray.py

# Diagnostic: list input devices
./venv/bin/python3 stt_listener.py --list-devices

# Diagnostic: show which text output method was auto-detected
./venv/bin/python3 stt_listener.py --detect-output

# Transcribe a specific audio file
./venv/bin/python3 stt_transcribe.py /path/to/file.wav
```

## Setup

```bash
./install.sh          # create venv, install deps, seed user config, install desktop entry
./setup_autostart.sh  # install and start systemd user service
./uninstall.sh        # full removal
```

The user config lives at `~/.config/speech-to-text/config.yaml`. `install.sh` seeds it from `config.yaml.example` on first install. The project root does NOT have an active config — `config.yaml` is gitignored and never read by the app.

## Architecture

The app splits into two processes that communicate over a Unix socket (`$XDG_RUNTIME_DIR/speech-to-text/daemon.sock`, with `STT_SOCKET_PATH` override):

**Daemon** (`stt_daemon.py` → `src/speech_to_text/daemon.py`)
- Headless, no GTK dependency
- Owns: evdev hotkey listener, arecord audio capture, faster-whisper transcription, text output
- Exposes a newline-delimited JSON IPC socket; broadcasts state/transcription/error events, accepts commands (`toggle_listening`, `set_listening`, `reload_config`, `get_state`, `set_log_level`, `cancel`, `clear_error`, `quit`)

**Tray** (`stt_tray.py` → `src/speech_to_text/tray.py` → `src/speech_to_text/gui/tray_app.py`)
- GTK/AppIndicator UI only — contains no recording or transcription logic
- Connects to the daemon via `DaemonClient` (IPC client in `gui/ipc_client.py`)
- The tray IPC client recovers a disconnected daemon via systemd or the shared launcher in `core/runtime.py`. `run.sh` waits for the supervised tray to register before opening its window.

**Key flow for a dictation:**
1. User holds hotkey → `HotkeyListener` (evdev) fires `start_recording` → `AudioRecorder` starts `arecord`
2. User releases hotkey → `stop_recording` → `AudioRecorder` stops arecord → the unique file enters a bounded queue processed by one worker
3. `_process()`: optional ffmpeg preprocessing → `Transcriber.transcribe_file()` → `TextOutput.type_text()`
4. Daemon saves history and broadcasts `transcription`; the tray updates the UI. Output failures remain recoverable from history.

## Text output (`core/text_output.py`)

`TextOutput` resolves the output method **once at `__init__`** and caches it in `_resolved_method`. With `method: auto` it probes candidates in order — critically, `wtype` is tested with a live subprocess call (not just binary presence) because GNOME Wayland has `wtype` installed but rejects it at runtime ("Compositor does not support the virtual keyboard protocol"). The selected method is logged at startup: `"Text output method: ydotool (auto-detected)"`.

On GNOME Wayland, `ydotool` is the correct tool. `wtype` requires `zwp_virtual_keyboard_manager_v1` which GNOME does not support.

## Config

`Config` reads from `~/.config/speech-to-text/config.yaml` only, merged over hardcoded defaults in `core/config.py`. When settings are saved via the GUI, the tray saves the file and sends `reload_config` to the daemon; the daemon's `reload_config` handler validates config, joins the old listener, updates the recorder, and preserves the transcriber/output unless their settings change. Busy reloads are rejected. Config writes use atomic replacement.

## Input group requirement

Reading `/dev/input/event*` requires the user to be in the `input` group. `run.sh` uses `sg input` to ensure the group is active even if the user was added after login. The systemd service inherits the session's groups directly.

## UI and verification

The user requires a dark-only UI. `gui/main_window.py` owns the Dictation, History, Settings, and Diagnostics pages. Keep GTK dialogs and popup controls dark as well.

Use `PYTHONPATH=src ./venv/bin/python3 -m unittest discover -s tests -v` for regression tests and `PYTHONPATH=src xvfb-run -a ./venv/bin/python3 tests/gui_smoke.py` for isolated GTK checks. See `docs/review.md` and `docs/handy-comparison.md` for rationale and verified boundaries.
