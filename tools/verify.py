#!/usr/bin/env python3
"""Run proportional repository verification without installing or benchmarking."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any


SCHEMA = 1
MODES = ("ui-layout", "ui-behavior", "full")
BACKENDS = ("cpu", "vulkan")
PROVENANCE_ENV = (
    "RUSTFLAGS",
    "CARGO_PROFILE_RELEASE_LTO",
    "TRANSCRIBE_CMAKE_ARGS",
    "VULKAN_SDK",
    "LIBRARY_PATH",
)


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def git_inputs(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
    )
    paths = []
    for raw in result.stdout.split(b"\0"):
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        if any(part in {".git", "target", "artifacts"} for part in relative.parts):
            continue
        path = root / relative
        if path.is_file():
            paths.append(relative)
    return sorted(paths)


def command_output(command: list[str], root: Path) -> str:
    result = subprocess.run(
        command,
        cwd=root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return result.stdout.strip()


def source_fingerprint(root: Path, backend: str, target_dir: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    inputs = git_inputs(root)
    for relative in inputs:
        digest.update(os.fsencode(relative.as_posix()))
        digest.update(b"\0")
        digest.update((root / relative).read_bytes())
        digest.update(b"\0")
    rustc = command_output(["rustc", "-Vv"], root)
    cargo = command_output(["cargo", "-V"], root)
    environment = {name: os.environ.get(name, "") for name in PROVENANCE_ENV}
    payload: dict[str, Any] = {
        "schema": SCHEMA,
        "backend": backend,
        "features": [] if backend == "cpu" else ["vulkan"],
        "profile": "release",
        "target_dir": str(target_dir),
        "build_flags": {
            "profile.release": "opt-level=3,lto=thin,codegen-units=1,strip=true"
        },
        "rustc": rustc,
        "cargo": cargo,
        "environment": environment,
        "inputs": [path.as_posix() for path in inputs],
    }
    payload["fingerprint"] = digest.hexdigest()
    return payload


def cargo_command(arguments: list[str], backend: str) -> list[str]:
    command = ["cargo", *arguments]
    if backend == "vulkan":
        insert_at = len(command)
        if "--" in command:
            insert_at = command.index("--")
        command[insert_at:insert_at] = ["--features", "vulkan"]
    return command


def mode_steps(mode: str, backend: str, binary: Path) -> list[tuple[str, list[str], int]]:
    steps: list[tuple[str, list[str], int]] = []
    if mode == "full":
        steps.extend(
            [
                ("cargo-test", cargo_command(["test", "--locked"], backend), 900),
                (
                    "cargo-clippy",
                    cargo_command(
                        ["clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
                        backend,
                    ),
                    900,
                ),
                (
                    "python-tests",
                    [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
                    300,
                ),
            ]
        )
    else:
        steps.extend(
            [
                (
                    "ui-tests",
                    cargo_command(["test", "--locked", "--bin", "voice-dictation"], backend),
                    900,
                ),
                (
                    "ui-clippy",
                    cargo_command(
                        ["clippy", "--locked", "--bin", "voice-dictation", "--", "-D", "warnings"],
                        backend,
                    ),
                    900,
                ),
            ]
        )
        if mode == "ui-behavior":
            steps.append(
                (
                    "python-tests",
                    [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
                    300,
                )
            )
    steps.extend(
        [
            ("release-build", cargo_command(["build", "--release", "--locked"], backend), 1800),
            ("native-probe", [sys.executable, "tools/native_probe.py", "--binary", str(binary)], 600),
        ]
    )
    return steps


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2) + "\n")


def copy_release_artifacts(build_target: Path, artifact_dir: Path) -> None:
    artifact_dir.mkdir(parents=True, exist_ok=True)
    for name in ("voice-dictation", "speech-service"):
        source = build_target / name
        if not source.is_file():
            raise FileNotFoundError(f"release artifact not found: {source}")
        shutil.copy2(source, artifact_dir / name)


def artifact_hashes(artifact_dir: Path) -> dict[str, str]:
    return {
        name: hashlib.sha256((artifact_dir / name).read_bytes()).hexdigest()
        for name in ("voice-dictation", "speech-service")
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", nargs="?", choices=MODES, default="ui-layout")
    parser.add_argument("--backend", choices=BACKENDS, default="cpu")
    parser.add_argument("--dry-run", action="store_true", help="print selected checks without running them")
    parser.add_argument("--no-reuse", action="store_true", help="force a fresh release artifact")
    parser.add_argument("--full", action="store_true", help="escalate any mode to the full integration gate")
    args = parser.parse_args()
    mode = "full" if args.full else args.mode
    root = Path(__file__).resolve().parents[1]
    configured_target = os.environ.get("CARGO_TARGET_DIR")
    target_dir = (
        Path(configured_target)
        if configured_target
        else root / "artifacts" / "verification" / "target" / args.backend
    )
    if not target_dir.is_absolute():
        target_dir = root / target_dir
    artifact_dir = root / "artifacts" / "verification" / "bin" / args.backend
    binary = artifact_dir / "voice-dictation"
    report_dir = root / "artifacts" / "verification"
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    report_path = report_dir / f"{stamp}-{mode}-{args.backend}.json"
    manifest_path = report_dir / f"provenance-{args.backend}.json"
    provenance = source_fingerprint(root, args.backend, target_dir)
    report: dict[str, Any] = {
        "schema": SCHEMA,
        "mode": mode,
        "backend": args.backend,
        "dry_run": args.dry_run,
        "started_at": now(),
        "target_dir": str(target_dir),
        "artifact_dir": str(artifact_dir),
        "binary": str(binary),
        "provenance": provenance,
        "steps": [],
    }
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    steps = mode_steps(mode, args.backend, binary)
    print(f"Verification mode: {mode} (backend: {args.backend})", flush=True)
    print(f"Target directory: {target_dir}", flush=True)
    if args.dry_run:
        print("Dry run: no commands will execute and no installation will occur.", flush=True)

    for name, command, timeout in steps:
        if name == "release-build":
            existing = None
            if manifest_path.is_file():
                try:
                    existing = json.loads(manifest_path.read_text())
                except (OSError, json.JSONDecodeError):
                    existing = None
            reusable = (
                not args.no_reuse
                and not args.dry_run
                and binary.is_file()
                and (artifact_dir / "speech-service").is_file()
                and isinstance(existing, dict)
                and existing.get("fingerprint") == provenance["fingerprint"]
                and existing.get("binary_sha256") == artifact_hashes(artifact_dir)
            )
            if reusable:
                print(f"[reuse] {name}: {binary}", flush=True)
                report["steps"].append(
                    {"name": name, "command": command, "status": "reused", "duration_seconds": 0.0}
                )
                continue
        printable = " ".join(command)
        print(f"[{('plan' if args.dry_run else 'run')}] {name}: {printable}", flush=True)
        if args.dry_run:
            report["steps"].append(
                {"name": name, "command": command, "status": "planned", "duration_seconds": 0.0}
            )
            continue
        log_path = report_dir / f"{stamp}-{mode}-{args.backend}-{name}.log"
        started = time.monotonic()
        status = "passed"
        exit_code = 0
        error = ""
        try:
            with log_path.open("w") as log:
                completed = subprocess.run(
                    command,
                    cwd=root,
                    env=env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    timeout=timeout,
                    check=False,
                )
            exit_code = completed.returncode
            if exit_code:
                status = "failed"
        except subprocess.TimeoutExpired:
            status = "timeout"
            exit_code = 124
            error = f"timed out after {timeout} seconds"
        except OSError as exc:
            status = "error"
            exit_code = 127
            error = str(exc)
        duration = time.monotonic() - started
        entry: dict[str, Any] = {
            "name": name,
            "command": command,
            "status": status,
            "exit_code": exit_code,
            "duration_seconds": round(duration, 3),
            "log": str(log_path),
        }
        if error:
            entry["error"] = error
        report["steps"].append(entry)
        write_report(report_path, report)
        if status != "passed":
            report["result"] = "failed"
            report["finished_at"] = now()
            write_report(report_path, report)
            print(f"Verification failed in {name}. See {log_path}", file=sys.stderr, flush=True)
            return 1
        if name == "release-build":
            try:
                copy_release_artifacts(target_dir / "release", artifact_dir)
            except OSError as exc:
                report["steps"][-1]["status"] = "error"
                report["steps"][-1]["error"] = str(exc)
                report["result"] = "failed"
                report["finished_at"] = now()
                write_report(report_path, report)
                print(
                    f"Verification failed while copying release artifacts: {exc}",
                    file=sys.stderr,
                    flush=True,
                )
                return 1
            manifest = {
                **provenance,
                "binary": str(binary),
                "artifact_dir": str(artifact_dir),
                "binary_sha256": artifact_hashes(artifact_dir),
                "created_at": now(),
            }
            write_report(manifest_path, manifest)

    report["result"] = "planned" if args.dry_run else "passed"
    report["finished_at"] = now()
    write_report(report_path, report)
    print(f"Verification {report['result']}. Report: {report_path}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
