#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_WAS_OPEN=false

if pgrep -x voice-dictation >/dev/null; then
  DESKTOP_WAS_OPEN=true
fi

"$SCRIPT_DIR/install.sh" "$@"
trap 'python3 "$SCRIPT_DIR/tools/installation.py" restart-failed || true' ERR
python3 "$SCRIPT_DIR/tools/installation.py" restart-start

if [[ "$DESKTOP_WAS_OPEN" == true ]]; then
  pkill -TERM -x voice-dictation || true
  for _ in {1..50}; do
    pgrep -x voice-dictation >/dev/null || break
    sleep 0.1
  done
fi

if systemctl --user cat speech-to-text-daemon.service >/dev/null 2>&1; then
  # Let the coordinator drain capture/history before systemd kills remaining children.
  UNIT_DROPIN_DIR="$HOME/.config/systemd/user/speech-to-text-daemon.service.d"
  mkdir -p "$UNIT_DROPIN_DIR"
  cat > "$UNIT_DROPIN_DIR/worker-lifecycle.conf" <<'EOF'
[Service]
KillMode=mixed
MemorySwapMax=0
EOF
  systemctl --user daemon-reload
  systemctl --user restart speech-to-text-daemon.service
else
  "$SCRIPT_DIR/setup_autostart.sh"
fi

if [[ "$DESKTOP_WAS_OPEN" == true ]]; then
  setsid -f "$SCRIPT_DIR/run.sh" >/dev/null 2>&1
fi

systemctl --user is-active --quiet speech-to-text-daemon.service
python3 "$SCRIPT_DIR/tools/installation.py" restart-result
echo "Local Voice Dictation build installed and service restarted."
if [[ "$DESKTOP_WAS_OPEN" == true ]]; then
  echo "Desktop reopened."
fi
