# Verification workflow

The project has two different verification needs:

1. A task check answers, "Did this change work?"
2. The full integration gate answers, "Does the combined release state still
   work?"

Running the full gate for every small edit makes the first question needlessly
expensive. Use the risk tier that matches the behavior changed, then run the
full gate for integration, release, and cross-cutting work.

## Tiers

| Change | Default checks | Escalate when |
|---|---|---|
| Documentation only | Relevant links, examples, and formatting | Commands, installation, or behavior claims changed |
| UI layout only | `ui-layout` | Shared behavior, accessibility, adapter contracts, or persistence changed |
| UI behavior or adapter | `ui-behavior` | Daemon, IPC, threading, config, or history behavior changed |
| Daemon, inference, persistence, configuration | `full` | Always consider the backend matrix when the backend changed |
| Dependencies, installer, acceleration, release | `full` plus the affected backend | Default CPU dependency closure or supported-host behavior changed |

Risk follows the behavior and dependency boundary, not the number of changed
lines or the file extension. An unknown or cross-cutting change falls back to
`full`.

## Runner

The runner is terminal-only and defaults to the lean CPU build:

```sh
python3 tools/verify.py ui-layout
python3 tools/verify.py ui-behavior
python3 tools/verify.py full
```

Useful options:

```sh
python3 tools/verify.py ui-layout --dry-run
python3 tools/verify.py ui-layout --no-reuse
python3 tools/verify.py ui-behavior --backend vulkan
python3 tools/verify.py ui-layout --full
```

`--full` is an explicit escalation to the integration gate. `--backend vulkan`
selects a separate provenance-checked artifact directory and is never implied by
a UI task. By default, each backend builds in its own directory under
`artifacts/verification/target/`, so verification cannot replace the locally
installed binaries in `target/release`. Set `CARGO_TARGET_DIR` explicitly when a
different isolated build directory is required.
The runner does not call `update_local.sh`, start a daemon, run benchmarks, or
send notifications.

Each run records selected checks, skips, command durations, exit status, logs,
and artifact provenance under the ignored `artifacts/verification/` directory.
CPU and Vulkan release artifacts use different target directories so one build
cannot silently masquerade as the other. Reuse is allowed only when the
provenance fingerprint includes the project inputs, untracked non-ignored
inputs, lockfile, Rust toolchain, feature set, target directory, build flags,
and relevant backend environment. Runtime evidence still belongs to the exact
binary and fixture invocation that produced it.

The runner reuses an exact release artifact, but it does not claim that a prior
test result proves a new change. Tests remain cheap enough to rerun; the build
and native probe are the main reusable steps.

## Full integration gate

The full gate remains:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s tests -v
cargo build --release --locked
python3 tools/native_probe.py
```

For a Vulkan release or backend change, repeat the relevant build and native
probe with `--backend vulkan` and the required SDK environment. A GPU build is
not part of the default CPU installation.

## Failure handling

Keep the runner report, logs, screenshots, exact binary path, and invocation.
First classify a failure as an application regression, harness defect,
environment problem, or unknown. Do not weaken an assertion merely to make the
runner green. If a probe helper changes, rerun the affected probe and review the
helper change separately from the product claim.

## Measuring the improvement

Track edit-to-handoff time by tier and warm or cold build state. The intended
outcome is near-zero repeated builds for unchanged provenance, zero Vulkan runs
for layout-only tasks, normally one native session after the final edit, and no
increase in missed regressions, escaped regressions, or user-data mutations.
