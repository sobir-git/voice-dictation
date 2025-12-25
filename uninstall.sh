#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_DIR="$HOME/.config/speech-to-text"
DATA_DIR="$HOME/.local/share/speech-to-text"
APP_NAME="speech-to-text"
UNIT_NAME="${APP_NAME}.service"
SYSTEMD_DIR="$HOME/.config/systemd/user"
AUTOSTART_FILE="$HOME/.config/autostart/${APP_NAME}.desktop"
DESKTOP_LAUNCHER="$HOME/.local/share/applications/${APP_NAME}.desktop"

echo "==================================="
echo "Speech-to-Text Uninstallation"
echo "==================================="
echo

read -p "This will remove configuration and data. Continue? [y/N] " -n 1 -r
echo

if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Uninstallation cancelled"
    exit 0
fi

if systemctl --user is-active "$UNIT_NAME" >/dev/null 2>&1; then
    echo "Stopping service..."
    systemctl --user stop "$UNIT_NAME" || true
fi

if systemctl --user is-enabled "$UNIT_NAME" >/dev/null 2>&1; then
    echo "Disabling service..."
    systemctl --user disable "$UNIT_NAME" || true
fi

if [[ -f "$SYSTEMD_DIR/$UNIT_NAME" ]]; then
    echo "Removing systemd service..."
    rm -f "$SYSTEMD_DIR/$UNIT_NAME"
    systemctl --user daemon-reload || true
fi

if [[ -d "$CONFIG_DIR" ]]; then
    echo "Removing config directory..."
    rm -rf "$CONFIG_DIR"
fi

if [[ -d "$DATA_DIR" ]]; then
    echo "Removing data directory..."
    rm -rf "$DATA_DIR"
fi

if [[ -f "$AUTOSTART_FILE" ]]; then
    echo "Removing autostart desktop entry..."
    rm -f "$AUTOSTART_FILE"
fi

if [[ -f "$DESKTOP_LAUNCHER" ]]; then
    echo "Removing desktop launcher..."
    rm -f "$DESKTOP_LAUNCHER"
fi

if [[ -d "$SCRIPT_DIR/venv" ]]; then
    echo "Removing virtual environment..."
    rm -rf "$SCRIPT_DIR/venv"
fi

echo
echo "Uninstallation complete!"

echo "The project directory is still at: $SCRIPT_DIR"
echo "Remove it manually if desired: rm -rf $SCRIPT_DIR"
