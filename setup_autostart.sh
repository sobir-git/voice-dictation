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
cat > "$SYSTEMD_DIR/speech-to-text.service" <<EOF
[Unit]
Description=Voice Dictation tray
PartOf=graphical-session.target
Wants=speech-to-text-daemon.service
After=graphical-session.target speech-to-text-daemon.service

[Service]
Type=simple
WorkingDirectory=$UNIT_DIR
ExecStart="$UNIT_DIR/run.sh" --background
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
EOF
systemd-analyze --user verify "$SYSTEMD_DIR/speech-to-text-daemon.service" "$SYSTEMD_DIR/speech-to-text.service"
systemctl --user daemon-reload
# Remove the old default.target link when upgrading a previous installation.
systemctl --user disable speech-to-text.service >/dev/null 2>&1 || true
systemctl --user enable speech-to-text-daemon.service speech-to-text.service
systemctl --user restart speech-to-text-daemon.service speech-to-text.service
rm -f "$HOME/.config/autostart/speech-to-text.desktop"
echo 'Voice Dictation services installed and started.'
