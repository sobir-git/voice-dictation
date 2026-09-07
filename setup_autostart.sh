#!/bin/bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SYSTEMD_DIR="$HOME/.config/systemd/user"
mkdir -p "$SYSTEMD_DIR"
# systemd expands % specifiers even in quotes.
UNIT_DIR="${SCRIPT_DIR//%/%%}"
cat > "$SYSTEMD_DIR/speech-to-text-daemon.service" <<EOF
[Unit]
Description=Voice Dictation speech service
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
WorkingDirectory=$UNIT_DIR
ExecStart="$UNIT_DIR/run.sh" --daemon
Restart=on-failure
RestartSec=2
TimeoutStopSec=10

[Install]
WantedBy=graphical-session.target
EOF
# The desktop is opened from the launcher; only dictation autostarts.
systemctl --user disable --now speech-to-text.service >/dev/null 2>&1 || true
rm -f "$SYSTEMD_DIR/speech-to-text.service"
systemd-analyze --user verify "$SYSTEMD_DIR/speech-to-text-daemon.service"
systemctl --user daemon-reload
systemctl --user enable --now speech-to-text-daemon.service
rm -f "$HOME/.config/autostart/speech-to-text.desktop"
echo 'Voice Dictation service installed. Open the desktop with run.sh.'
