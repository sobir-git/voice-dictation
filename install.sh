#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "Building the native desktop and speech service..."
command -v cargo >/dev/null
command -v cmake >/dev/null
command -v ffmpeg >/dev/null
command -v arecord >/dev/null

echo "Installing default config..."
CONFIG_DIR="$HOME/.config/speech-to-text"
CONFIG_FILE="$CONFIG_DIR/config.yaml"
if [[ ! -f "$CONFIG_FILE" ]]; then
  mkdir -p "$CONFIG_DIR"
  cp "$SCRIPT_DIR/config.yaml.example" "$CONFIG_FILE"
  echo "  Created $CONFIG_FILE — edit this file to customize settings."
else
  echo "  Config already exists at $CONFIG_FILE — skipping."
fi

if [[ -z "${VOICE_DICTATION_FEATURES+x}" ]] \
  && [[ -x "$SCRIPT_DIR/target/release/speech-service" ]] \
  && "$SCRIPT_DIR/target/release/speech-service" --list-optimizations 2>/dev/null \
    | grep -q '"vulkan_build": true'; then
  export VOICE_DICTATION_FEATURES=vulkan
  echo "Preserving the installed Vulkan backend."
fi

# Build selection belongs here so direct installs and update_local agree.
BUILD_ARGS=(--manifest-path "$SCRIPT_DIR/Cargo.toml" --locked --release)
if [[ -n "${VOICE_DICTATION_FEATURES:-}" ]]; then
  BUILD_ARGS+=(--features "$VOICE_DICTATION_FEATURES")
fi
cargo build "${BUILD_ARGS[@]}"

# Verification may build in an isolated target directory. Install those exact
# binaries where run.sh starts them, rather than leaving an older local build.
BUILD_TARGET="${CARGO_TARGET_DIR:-$SCRIPT_DIR/target}"
if [[ "$BUILD_TARGET" != /* ]]; then
  BUILD_TARGET="$PWD/$BUILD_TARGET"
fi
if [[ "$(realpath -m "$BUILD_TARGET")" != "$(realpath -m "$SCRIPT_DIR/target")" ]]; then
  mkdir -p "$SCRIPT_DIR/target/release"
  for binary in voice-dictation speech-service; do
    staged="$(mktemp "$SCRIPT_DIR/target/release/$binary.XXXXXX")"
    if ! install -m 755 "$BUILD_TARGET/release/$binary" "$staged"; then
      rm -f "$staged"
      exit 1
    fi
    mv -f "$staged" "$SCRIPT_DIR/target/release/$binary"
  done
fi

echo "Installing desktop entry..."
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"
"$SCRIPT_DIR/install_icons.sh"

cat > "$DESKTOP_DIR/speech-to-text.desktop" << EOF
[Desktop Entry]
Type=Application
Name=Voice Dictation
Comment=Fast, local streaming voice dictation
Exec="$SCRIPT_DIR/run.sh"
Icon=voice-dictation
Terminal=false
Categories=Utility;AudioVideo;
StartupNotify=false
EOF

echo ""
echo "Install complete."
echo ""
echo "You can now:"
echo "  - Run from terminal: $SCRIPT_DIR/run.sh"
echo "  - Launch from applications menu: 'Voice Dictation'"
echo "  - Enable autostart: $SCRIPT_DIR/setup_autostart.sh"
