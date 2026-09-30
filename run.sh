#!/bin/bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export VOICE_DICTATION_PROJECT="$SCRIPT_DIR"
COMPONENT=desktop
if [[ "${1:-}" == '--daemon' ]]; then COMPONENT=daemon; fi
RELEASE="$(python3 "$SCRIPT_DIR/tools/installation.py" resolve --component "$COMPONENT")"
if [[ "${1:-}" == '--daemon' ]]; then
  shift
  STT_ARGS=("$RELEASE/speech-service" "$@")
else
  STT_LOCK_DIR="${XDG_RUNTIME_DIR:-/tmp/speech-to-text-$(id -u)}/speech-to-text"
  umask 077
  mkdir -p "$STT_LOCK_DIR"
  exec 9>"$STT_LOCK_DIR/desktop.lock"
  if ! flock -n 9; then
    if command -v xdotool >/dev/null; then
      xdotool search --onlyvisible --name '^Voice Dictation$' windowactivate >/dev/null 2>&1 || true
    fi
    exit 0
  fi
  STT_ARGS=("$RELEASE/voice-dictation" "$@")
fi
if [[ " $(id -nG) " == *" input "* ]]; then
  exec "${STT_ARGS[@]}"
fi
printf -v STT_COMMAND '%q ' "${STT_ARGS[@]}"
exec sg input -c "exec $STT_COMMAND"
