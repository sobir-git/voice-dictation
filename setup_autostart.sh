#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="speech-to-text"
UNIT_NAME="${APP_NAME}.service"
AUTOSTART_DIR="$HOME/.config/autostart"
AUTOSTART_FILE="$AUTOSTART_DIR/${APP_NAME}.desktop"
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

DESKTOP_CONTENT="[Desktop Entry]
Type=Application
Name=Speech-to-Text
Exec=sh -lc 'cd \"$SCRIPT_DIR\" && if [ -x ./venv/bin/python3 ]; then ./venv/bin/python3 stt_tray.py; else python3 stt_tray.py; fi'
Terminal=false
Categories=Utility;
"

echo "Installing systemd user service..."
mkdir -p "$SYSTEMD_DIR"
echo "$SERVICE_CONTENT" > "$SYSTEMD_DIR/$UNIT_NAME"

systemctl --user daemon-reload
systemctl --user enable --now "$UNIT_NAME" || true

echo "Installing XDG autostart entry..."
mkdir -p "$AUTOSTART_DIR"
echo "$DESKTOP_CONTENT" > "$AUTOSTART_FILE"

echo "Done."
