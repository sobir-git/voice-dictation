#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "Building the native desktop and speech service..."
command -v cargo >/dev/null
command -v cmake >/dev/null
command -v ffmpeg >/dev/null
command -v arecord >/dev/null

echo "Installing default config..."
CONFIG_DIR="$HOME/.config/speech-to-text"
CONFIG_FILE="$CONFIG_DIR/config.yaml"
if [[ ! -f "$CONFIG_FILE" ]]; then
  mkdir -p "$CONFIG_DIR"
  cp "$SCRIPT_DIR/config.yaml.example" "$CONFIG_FILE"
  echo "  Created $CONFIG_FILE — edit this file to customize settings."
else
  echo "  Config already exists at $CONFIG_FILE — skipping."
fi

cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml" --locked --release

echo "Installing desktop entry..."
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"

cat > "$DESKTOP_DIR/speech-to-text.desktop" << EOF
[Desktop Entry]
Type=Application
Name=Voice Dictation
Comment=Voice dictation using Whisper
Exec="$SCRIPT_DIR/run.sh"
Icon=audio-input-microphone
Terminal=false
Categories=Utility;AudioVideo;
StartupNotify=false
EOF

echo ""
echo "Install complete."
echo ""
echo "You can now:"
echo "  - Run from terminal: $SCRIPT_DIR/run.sh"
echo "  - Launch from applications menu: 'Voice Dictation'"
echo "  - Enable autostart: $SCRIPT_DIR/setup_autostart.sh"
