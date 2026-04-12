#!/bin/bash
# Wrapper that ensures the input group is active before launching the tray app.
# Needed when the user session was started before being added to the input group.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec sg input -c "$SCRIPT_DIR/venv/bin/python3 $SCRIPT_DIR/stt_tray.py"
