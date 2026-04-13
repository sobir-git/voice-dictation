# Voice Dictation

A lightweight voice dictation tool that runs in the system tray and types transcriptions into the active application.

## Project Layout

- `src/speech_to_text/` contains the real application package
- `src/speech_to_text/core/` contains recording, transcription, hotkey, config, and output logic
- `src/speech_to_text/gui/` contains tray/UI helpers
- root-level `stt_*.py` files are thin compatibility wrappers so existing commands still work

## Features

- System tray app (GTK/AppIndicator) with:
  - Toggle listening
  - Settings
  - History
  - Quit
- Hold-to-record hotkey using `evdev`
- Transcription via `faster-whisper`
- Optional cursor indicator near the mouse pointer:
  - Recording: red
  - Processing: orange
- Optional audio preprocessing via `ffmpeg` (only used if installed)
- Autostart via systemd user service + XDG desktop entry

## Requirements

### System packages

- Python 3
- `arecord` (usually from `alsa-utils`)
- `ffmpeg` (optional)
- PyGObject + GTK3 + AppIndicator:
  - `python3-gi`
  - `python3-gi-cairo`
  - `gir1.2-gtk-3.0`
  - `gir1.2-ayatanaappindicator3-0.1`

### Python packages

Installed via `requirements.txt` into a venv.

## Install

```bash
./install.sh
```

This will:
- Create a venv (with `--system-site-packages` so `gi` works)
- Install Python deps
- Install a desktop entry into `~/.local/share/applications/`

## Run

```bash
./venv/bin/python3 stt_tray.py
```

The wrapper loads the package from `src/speech_to_text/`.

## Autostart

```bash
./setup_autostart.sh
```

## Configuration

Copy the example config into the app config directory and edit it:

```bash
mkdir -p ~/.config/speech-to-text
cp config.yaml.example ~/.config/speech-to-text/config.yaml
```

The app reads config from `~/.config/speech-to-text/config.yaml`.

## Uninstall

```bash
./uninstall.sh
```
