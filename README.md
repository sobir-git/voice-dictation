# Voice Dictation

A lightweight voice dictation tool that runs in the system tray and types transcriptions into the active application.

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

## Autostart

```bash
./setup_autostart.sh
```

## Configuration

Copy the example config and edit it:

```bash
cp config.yaml.example config.yaml
```

`config.yaml` is ignored by git.

## Uninstall

```bash
./uninstall.sh
```
