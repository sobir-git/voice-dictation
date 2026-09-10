#!/usr/bin/env bash
set -euo pipefail
research_root=${1:-/tmp/canary-vulkan-research}
script_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
sdk_root="$research_root/1.4.357.1/x86_64"
if [[ ! -x "$sdk_root/bin/glslc" ]]; then
  echo "Missing isolated LunarG 1.4.357.1 compiler: $sdk_root/bin/glslc" >&2
  exit 1
fi
mkdir -p "$research_root/src"
if [[ ! -d "$research_root/native" ]]; then
  crate_root=/home/fire/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/transcribe-cpp-sys-0.2.3
  cp -a -- "$crate_root" "$research_root/native"
  patch -p1 -d "$research_root/native" < "$script_root/native.patch"
else
  # Refuse to silently overwrite an existing experiment.
  patch --dry-run -R -p1 -d "$research_root/native" < "$script_root/native.patch" > /dev/null
fi
cp -- "$script_root/experiment.Cargo.toml" "$research_root/Cargo.toml"
cp -- "$script_root/experiment.Cargo.lock" "$research_root/Cargo.lock"
cp -- "$script_root/driver.rs" "$research_root/src/main.rs"
cp -- "$script_root/experiment.build.rs" "$research_root/build.rs"
export LD_LIBRARY_PATH="$research_root/deps/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export TRANSCRIBE_CMAKE_ARGS="-DVulkan_INCLUDE_DIR=$sdk_root/include -DVulkan_LIBRARY=/usr/lib/x86_64-linux-gnu/libvulkan.so.1 -DVulkan_GLSLC_EXECUTABLE=$sdk_root/bin/glslc -DCMAKE_PREFIX_PATH=$sdk_root"
export CARGO_TARGET_DIR="$research_root/target"
export CARGO_BUILD_JOBS=6
cargo build --release --locked --manifest-path "$research_root/Cargo.toml"
