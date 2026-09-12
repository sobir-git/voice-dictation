#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable"

mkdir -p "$ICON_DIR/apps" "$ICON_DIR/status"
cp "$SCRIPT_DIR/icons/proposal/voice-dictation-ready.svg" "$ICON_DIR/apps/voice-dictation.svg"
for state in ready recording transcribing paused error; do
  cp "$SCRIPT_DIR/icons/proposal/voice-dictation-$state.svg" \
    "$ICON_DIR/status/voice-dictation-$state.svg"
done

if command -v gtk-update-icon-cache >/dev/null; then
  gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Voice Dictation icons installed."
