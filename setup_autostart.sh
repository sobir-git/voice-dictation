#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="speech-to-text"
UNIT_NAME="${APP_NAME}.service"
SYSTEMD_DIR="$HOME/.config/systemd/user"

PYTHON_CMD="python3"
if [[ -x "$SCRIPT_DIR/venv/bin/python3" ]]; then
  PYTHON_CMD="$SCRIPT_DIR/venv/bin/python3"
fi

SERVICE_CONTENT="[Unit]
Description=Speech-to-Text Tray
After=graphical-session.target

[Service]
Type=simple
WorkingDirectory=$SCRIPT_DIR
ExecStart=$PYTHON_CMD $SCRIPT_DIR/stt_tray.py
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"

echo "Installing systemd user service..."
mkdir -p "$SYSTEMD_DIR"
echo "$SERVICE_CONTENT" > "$SYSTEMD_DIR/$UNIT_NAME"

systemctl --user daemon-reload
systemctl --user enable --now "$UNIT_NAME" || true

if [[ -f "$HOME/.config/autostart/${APP_NAME}.desktop" ]]; then
  echo "Removing old XDG autostart entry..."
  rm -f "$HOME/.config/autostart/${APP_NAME}.desktop"
fi

echo "Done."
