#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_WAS_OPEN=false

if pgrep -x voice-dictation >/dev/null; then
  DESKTOP_WAS_OPEN=true
fi

if [[ -z "${VOICE_DICTATION_FEATURES+x}" ]] \
  && [[ -x "$SCRIPT_DIR/target/release/speech-service" ]] \
  && "$SCRIPT_DIR/target/release/speech-service" --list-optimizations 2>/dev/null \
    | grep -q '"vulkan_build": true'; then
  export VOICE_DICTATION_FEATURES=vulkan
  echo "Preserving the installed Vulkan backend."
fi

"$SCRIPT_DIR/install.sh"

if [[ "$DESKTOP_WAS_OPEN" == true ]]; then
  pkill -TERM -x voice-dictation || true
  for _ in {1..50}; do
    pgrep -x voice-dictation >/dev/null || break
    sleep 0.1
  done
fi

if systemctl --user cat speech-to-text-daemon.service >/dev/null 2>&1; then
  systemctl --user restart speech-to-text-daemon.service
else
  "$SCRIPT_DIR/setup_autostart.sh"
fi

if [[ "$DESKTOP_WAS_OPEN" == true ]]; then
  setsid -f "$SCRIPT_DIR/run.sh" >/dev/null 2>&1
fi

systemctl --user is-active --quiet speech-to-text-daemon.service
echo "Local Voice Dictation build installed and service restarted."
if [[ "$DESKTOP_WAS_OPEN" == true ]]; then
  echo "Desktop reopened."
fi
