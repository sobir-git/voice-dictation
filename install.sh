#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="speech-to-text"
VENV_DIR="$SCRIPT_DIR/venv"

PYTHON=python3
if ! command -v "$PYTHON" >/dev/null 2>&1; then
  echo "python3 not found"
  exit 1
fi

echo "Checking system dependencies..."
if ! python3 -c "import gi" 2>/dev/null; then
  echo ""
  echo "ERROR: PyGObject (gi) not found. Install it with:"
  echo "  sudo apt install python3-gi python3-gi-cairo gir1.2-gtk-3.0 gir1.2-ayatanaappindicator3-0.1"
  echo ""
  exit 1
fi

echo "Setting up venv with system packages..."
"$PYTHON" -m venv --system-site-packages "$VENV_DIR"

source "$VENV_DIR/bin/activate"

pip install --upgrade pip
pip install -r "$SCRIPT_DIR/requirements.txt"

echo "Making scripts executable..."
chmod +x "$SCRIPT_DIR/stt_tray.py" "$SCRIPT_DIR/stt_listener.py" "$SCRIPT_DIR/stt_transcribe.py" || true
chmod +x "$SCRIPT_DIR/stt_daemon.py" || true
chmod +x "$SCRIPT_DIR/setup_autostart.sh" "$SCRIPT_DIR/uninstall.sh" || true

echo "Installing desktop entry..."
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"

cat > "$DESKTOP_DIR/speech-to-text.desktop" << EOF
[Desktop Entry]
Type=Application
Name=Speech-to-Text
Comment=Voice dictation using Whisper
Exec=$VENV_DIR/bin/python3 $SCRIPT_DIR/stt_tray.py
Icon=audio-input-microphone
Terminal=false
Categories=Utility;AudioVideo;
StartupNotify=false
EOF

echo ""
echo "Install complete."
echo ""
echo "You can now:"
echo "  - Run from terminal: $VENV_DIR/bin/python3 $SCRIPT_DIR/stt_tray.py"
echo "  - Launch from applications menu: 'Speech-to-Text'"
echo "  - Enable autostart: $SCRIPT_DIR/setup_autostart.sh"
