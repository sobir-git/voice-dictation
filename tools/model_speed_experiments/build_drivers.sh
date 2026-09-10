#!/usr/bin/env bash
set -euo pipefail
# Requires the isolated native source and LunarG SDK described in README.md.
export LD_LIBRARY_PATH=/tmp/canary-vulkan-research/deps/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
export TRANSCRIBE_CMAKE_ARGS='-DVulkan_INCLUDE_DIR=/tmp/canary-vulkan-research/1.4.357.1/x86_64/include -DVulkan_LIBRARY=/usr/lib/x86_64-linux-gnu/libvulkan.so.1 -DVulkan_GLSLC_EXECUTABLE=/tmp/canary-vulkan-research/1.4.357.1/x86_64/bin/glslc -DCMAKE_PREFIX_PATH=/tmp/canary-vulkan-research/1.4.357.1/x86_64'
CARGO_TARGET_DIR=/tmp/canary-vulkan-research/target cargo build --release --locked --manifest-path /tmp/parakeet-research/Cargo.toml
CARGO_TARGET_DIR=/home/fire/projects/voice-dictation/target CARGO_BUILD_JOBS=6 cargo build --release --locked --offline --manifest-path /tmp/whisper-research/Cargo.toml
