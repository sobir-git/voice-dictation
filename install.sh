#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Publish the complete pair before changing launcher integration.
python3 "$SCRIPT_DIR/tools/installation.py" install "$@"

echo "Installing default config..."
CONFIG_DIR="$HOME/.config/speech-to-text"
CONFIG_FILE="$CONFIG_DIR/config.yaml"
if [[ ! -f "$CONFIG_FILE" ]]; then
  mkdir -p "$CONFIG_DIR"
  cp "$SCRIPT_DIR/config.yaml.example" "$CONFIG_FILE"
else
  echo "  Config already exists — skipping."
fi

echo "Installing desktop entry..."
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"
"$SCRIPT_DIR/install_icons.sh"

cat > "$DESKTOP_DIR/speech-to-text.desktop" << EOF
[Desktop Entry]
Type=Application
Name=Voice Dictation
Comment=Fast, local streaming voice dictation
Exec="$SCRIPT_DIR/run.sh"
Icon=voice-dictation
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
