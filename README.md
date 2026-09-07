# Voice Dictation

Local voice dictation for Linux, with a dark GTK desktop window and system tray. Hold your hotkey, speak, and release to type into the active application.

## Run

```bash
./run.sh
```

Open the tray menu to return to the window. Closing the window keeps dictation running. **Quit tray** closes the UI; the speech service remains available.

The window includes:

- **Dictation:** recording readiness, actual microphone levels, processing status, Cancel, and the latest transcription.
- **History:** locally saved transcriptions, search, and copy. The daemon saves history even while the tray is closed or text output fails.
- **Settings:** microphone selection and test, hotkey capture, model, language, preprocessing, and output preferences.
- **Diagnostics:** runtime report, dependency versions, microphone routing, debug level, live logs, and copy report.

## Installation

System packages include Python 3, `alsa-utils`, GTK 3/PyGObject, and AppIndicator. `ffmpeg` enables optional audio normalization. The microphone dropdown uses `pactl` to list PipeWire sources; explicit device selection uses the PipeWire ALSA plugin.

On Ubuntu/Debian:

```bash
sudo apt install python3-venv python3-gi python3-gi-cairo gir1.2-gtk-3.0 \
  gir1.2-ayatanaappindicator3-0.1 alsa-utils ffmpeg pulseaudio-utils pipewire-alsa xdotool
./install.sh
./setup_autostart.sh
```

Reading global hotkeys requires membership in the `input` group. `run.sh` activates that group when necessary. On GNOME Wayland, install and configure `ydotoold`; `wtype` does not support GNOME's compositor. X11 uses `xdotool` by default.

Autostart installs two user services:

- `speech-to-text-daemon.service`: recording, transcription, history, and text output.
- `speech-to-text.service`: the tray and desktop window.

Both start with the graphical session. The daemon restarts after a crash; the tray reconnects automatically.

## Configuration

The active config is `~/.config/speech-to-text/config.yaml`. Installation seeds it from `config.yaml.example`; the project-root `config.yaml` is never read.

Use **Settings → Input** to select a microphone for dictation. This does not change the system-wide default. The eight-second test uses audio in memory and does not save or transcribe it.

Settings apply after the daemon acknowledges a reload. Finish an active dictation before saving. Changing unrelated settings preserves the loaded model.

`audio.temp_file` specifies the directory and filename prefix for unique recording files. They are readable only by the user and removed after processing. A sudden process kill can leave an unfinished temporary file.

`output.method: none` saves text to History without typing. `xclip` copies to the clipboard only. Other methods type text and report command failures.

Whisper runs locally. An uncached model downloads on first use. Cached models load without an update check. Smaller models and beam size 1 generally trade some accuracy for less computation; existing user preferences are preserved.

## Debugging

Enable **Diagnostics → Detailed debug logging**, then **Follow logs** to watch a reproduction. Debug logs record the recording ID, first audio arrival, microphone signal, process/thread, processing times, and errors. The application does not log recognized text by default.

```bash
./venv/bin/python3 stt_listener.py --diagnose
./venv/bin/python3 stt_listener.py --list-microphones
./venv/bin/python3 stt_listener.py --list-devices
./venv/bin/python3 stt_listener.py --detect-output
./venv/bin/python3 stt_listener.py --cancel

journalctl --user -u speech-to-text-daemon.service -u speech-to-text.service -n 100
systemctl --user restart speech-to-text-daemon.service
```

Rotating logs are in `~/.local/share/speech-to-text/app.log` and `app-tray.log`. Each has three backups. Reports include local device names and paths; review before sharing.

If no text appears, test the microphone first. A virtual loopback or monitor can produce an entirely silent recording. If transcription succeeds but typing fails, recover the text from History.

Cancellation suppresses pending output. Native inference already running finishes before the processing state clears. Text already typed cannot be retracted.

## Architecture

The root `stt_*.py` commands load `src/speech_to_text/`.

The headless daemon owns evdev hotkeys, `arecord`, faster-whisper, ordered transcription, SQLite history, and output tools. It accepts newline-delimited JSON over a user-private Unix socket at `$XDG_RUNTIME_DIR/speech-to-text/daemon.sock`. Without `XDG_RUNTIME_DIR`, it uses `/tmp/speech-to-text-<uid>/speech-to-text/daemon.sock`. `STT_SOCKET_PATH` overrides the address for all clients and the daemon.

The GTK process handles presentation, settings, and explicit microphone tests. IPC commands include `set_listening`, `toggle_listening`, `reload_config`, `set_log_level`, `get_state`, `cancel`, `clear_error`, and `quit`.

## Verification

```bash
PYTHONPATH=src ./venv/bin/python3 -m unittest discover -s tests -v
PYTHONPATH=src xvfb-run -a ./venv/bin/python3 tests/gui_smoke.py
```

The second command requires Xvfb. It uses temporary configuration/history and does not touch the real microphone or type into the desktop.

See [review findings and validation](docs/review.md) and [Handy inspirations](docs/handy-comparison.md).

## Uninstall

```bash
./uninstall.sh
```
