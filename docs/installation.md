# Installation and build editions

Cargo output is development output. The launcher executes a canonical immutable
pair from `${XDG_DATA_HOME:-$HOME/.local/share}/speech-to-text/installations/releases/<install-id>/`.
`installations/current` is an atomically replaced symlink. Each release contains
`voice-dictation`, `speech-service`, and `receipt.json`. A running daemon resolves
its own executable and companion paths within the same release even after an
update switches `current`. Plain Cargo builds and verification never activate a
release. `run.sh` requires an installed pair and gives an installation instruction
when it is absent.

```sh
./install.sh                    # fresh CPU; otherwise preserve receipt edition
./update_local.sh               # install, restart daemon, reopen an open desktop
VOICE_DICTATION_FEATURES=vulkan ./update_local.sh  # explicit Vulkan selection
VOICE_DICTATION_FEATURES= ./update_local.sh        # explicit CPU downgrade
```

An unset feature selection preserves the installed receipt's edition, validated
against both pair hashes and the service's compiled capability. Only migration
without a receipt may inspect the legacy `target/release` pair. Runtime model
profiles do not select compiler features. Existing configuration, history, and
cached models are preserved. The template creates configuration only if absent.
Migration happens through install/update; the launcher never silently builds.

Installation checks the recording runtime tools `ffmpeg` and `arecord` before
building or publishing. Missing tools leave the previously selected release
active. Verified-artifact installation does not require Cargo or CMake.

Builds default to `artifacts/installation/target/<cpu|vulkan>/release/`.
When set, `CARGO_TARGET_DIR` is the installation **base** directory: installation
appends `cpu` or `vulkan` to keep editions isolated. Relative bases resolve from
the invoking working directory. The verification runner retains its existing
semantics: an explicit `CARGO_TARGET_DIR` is its exact build directory.
Only empty features or `vulkan` are accepted by installation.

## Explicit fixed AVX2 build

On compatible Linux x86-64 hosts, opt into the measured native CPU preset:

```sh
python3 tools/verify.py full --backend vulkan --cpu-isa avx2
VOICE_DICTATION_CPU_ISA=avx2 ./update_local.sh
# A fresh Vulkan source installation also needs VOICE_DICTATION_FEATURES=vulkan.
```

This retains Vulkan and optimizes the native CPU path, including CPU fallback.
It does not change the runtime profile, model, threads, streaming or configuration.
The host floor is SSE4.2, AVX, AVX2, FMA, F16C and BMI2, with OS-enabled AVX state.
Installation checks every Linux-exposed CPU's flags before executing an AVX2
candidate, including the backend-capability probe. Unsupported hosts receive a
clear error and keep the previous release. This is an explicit build edition,
not runtime ISA dispatch or a universal binary.

`tools/native_cpu.py` supplies the fixed recipe: conservative x86 mode OFF,
GGML_NATIVE OFF, the six instruction switches ON, and AVX512/VBMI/VNNI/BF16 and
AVX_VNNI OFF. It never adds `-march=native`. Conflicting CMake definitions,
compiler/Rust target overrides and unverified `TRANSCRIBE_DIR` prebuilts are
rejected for this preset. Unrelated CMake options remain available. Schema-2
provenance records `build_flags.cpu_isa`, `cpu_isa_floor` and hashes of the
controlling environment, so a changed preset invalidates artifact reuse.

Without an ISA selection, a fresh build retains the existing native defaults.
Subsequent source installs preserve the installed receipt's preset, independently
of its CPU/Vulkan backend. `VOICE_DICTATION_CPU_ISA=default` explicitly resets
that selection; clear any manually supplied low-level target flags as well.
Verification accepts `--cpu-isa default`, or the same environment selector.
Verified bundles use their own recorded preset; an explicit conflicting selector
is rejected. Original VM provenance remains in the installation receipt.

For a passed AVX2 Vulkan bundle, use the existing export/install flow above; no
laptop compilation is necessary. Keep the bundle and matching source together,
including untracked source inputs recorded in its manifest. See the
[measured stop-to-text comparison](performance-evidence/2026-10-02-avx2-stop-to-text.md)
for benefit, memory and quality limits.

## Verification and portable artifacts

```sh
python3 tools/verify.py full
python3 tools/verify.py full --backend vulkan
python3 tools/export_verified_artifact.py artifacts/verification/provenance-vulkan.json /path/to/vulkan-bundle
VOICE_DICTATION_FEATURES=vulkan ./update_local.sh --verified-artifact /path/to/vulkan-bundle/manifest.json
```

Export only after reviewing the gate logs and native screenshots. A bundle has
relative artifact paths, so the whole bundle can be moved from the VM to the
owner's laptop. Installation validates the portable source digest (including
dirty source), manifest provenance self-consistency, pair hashes, and actual
compiled capability. A full passed gate is required. Built, pending, stale,
missing, or mismatched artifacts are refused before activation. This boundary
uses the already verified binaries and does not compile again. The original VM
Rust/native toolchain, environment value hashes, flags, target, features, and
complete build fingerprint remain in the receipt; it never claims a local build.
Full verification is build/test/probe evidence, not proof of a particular GPU's
runtime acceleration. Validate the actual Intel runtime on the owner host.

Installation stages a complete pair and receipt, then switches one symlink.
Build/validation/publication failures before activation keep the previous pair
active. Previous releases remain available for running processes and inspection;
there is no automatic pruning. Clean uninstall removes releases through the
existing destructive confirmation. Treat installed files as read-only; external
manual replacement is detected when preserving the receipt edition.

## Causal diagnostics

`~/.local/share/speech-to-text/app.log` contains asynchronous structured daemon
records. Startup records identify the canonical executable, receipt installation
and build IDs, compiled backend, requested profile/model, and configuration path.
Model load stages include elapsed time, reason, whether native GPU loading was
attempted, and the exact actual profile. `backend_not_compiled` happens before
native GPU initialization. An attempted native GPU model load that fails is
recorded as `gpu_initialization_or_model_load_failed`; this boundary cannot
attribute a native failure solely to the driver. CPU fallback success explicitly
records `actual_profile: standard`.

`${XDG_DATA_HOME:-$HOME/.local/share}/speech-to-text/installation.jsonl` records
selection (`explicit`, `preserved_receipt`, `legacy_migration`, `fresh_cpu`),
build/validation outcome, artifact paths/hashes, source and toolchain identity,
release switches, canonical launches, and update restart observations. It rotates
at 1 MiB and retains five backups. Updates fail if the observed daemon executable
does not match the selected release. Logs omit raw environment, configuration
contents, audio, and transcripts. Paths and native errors can still contain host
metadata. No idle polling is added.

```sh
python3 tools/diagnostics.py --output /tmp/voice-dictation-diagnostics.zip
readlink -f "${XDG_DATA_HOME:-$HOME/.local/share}/speech-to-text/installations/current"
cat "${XDG_DATA_HOME:-$HOME/.local/share}/speech-to-text/installations/current/receipt.json"
```

Diagnostics discovers installed binaries and includes the receipt and rotated
installation logs. No receipt or historical installation event means
`historical_unknown`, not an invented cause for an older replacement. The observed
old laptop CPU-only executable explains the current pre-initialization fallback;
the exact older installation action was not logged and remains unknown.
