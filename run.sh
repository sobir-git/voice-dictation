#!/bin/bash
# Activate input permissions inherited by the daemon and tray.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_CMD="$SCRIPT_DIR/venv/bin/python3"
ENTRY="$SCRIPT_DIR/stt_tray.py"
if [[ "${1:-}" != '--daemon' && "${1:-}" != '--background' && -f "$HOME/.config/systemd/user/speech-to-text.service" ]] && command -v gdbus >/dev/null; then
  systemctl --user start speech-to-text.service
  gdbus wait --session --timeout=10 com.github.voice-dictation.stt-tray
fi
if [[ "${1:-}" == '--daemon' ]]; then
  ENTRY="$SCRIPT_DIR/stt_daemon.py"
  shift
fi
if [[ " $(id -nG) " == *" input "* ]]; then
  exec "$PYTHON_CMD" "$ENTRY" "$@"
fi
# bash %q quoting preserves spaces and shell metacharacters in paths/arguments.
printf -v STT_COMMAND '%q ' "$PYTHON_CMD" "$ENTRY" "$@"
exec sg input -c "$STT_COMMAND"
