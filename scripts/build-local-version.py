#!/usr/bin/env python3
"""Build Codex with a temporary local version stamp.

This wraps set-local-version.py for local/source builds that should report a
release-like version, then restores the workspace version files after the build
finishes or fails.
"""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


VERSION_FILE_PATHS = (
    Path("codex-rs/Cargo.toml"),
    Path("codex-rs/Cargo.lock"),
)


@dataclass
class FileSnapshot:
    path: Path
    contents: bytes
    mode: int


def run(args: list[str], cwd: Path) -> None:
    print(f"+ {shlex.join(args)}", flush=True)
    subprocess.run(args, cwd=cwd, check=True)


def repo_root() -> Path:
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return Path(result.stdout.strip())


def snapshot_files(repo: Path) -> list[FileSnapshot]:
    snapshots: list[FileSnapshot] = []
    for relative_path in VERSION_FILE_PATHS:
        path = repo / relative_path
        snapshots.append(
            FileSnapshot(
                path=path,
                contents=path.read_bytes(),
                mode=path.stat().st_mode,
            )
        )
    return snapshots


def restore_files(snapshots: list[FileSnapshot]) -> None:
    for snapshot in snapshots:
        snapshot.path.write_bytes(snapshot.contents)
        os.chmod(snapshot.path, snapshot.mode)
    restored = ", ".join(str(snapshot.path) for snapshot in snapshots)
    print(f"Restored {restored}.", flush=True)


def split_args(argv: list[str]) -> tuple[list[str], list[str]]:
    if "--" not in argv:
        return argv, []
    separator = argv.index("--")
    return argv[:separator], argv[separator + 1 :]


def parse_args(argv: list[str]) -> tuple[argparse.Namespace, list[str], list[str]]:
    wrapper_and_version_args, cargo_extra_args = split_args(argv)
    parser = argparse.ArgumentParser(
        description=(
            "Temporarily run set-local-version.py, build codex-cli, then restore "
            "codex-rs/Cargo.toml and codex-rs/Cargo.lock. Arguments before '--' "
            "are passed to set-local-version.py; arguments after '--' are "
            "appended to cargo build."
        )
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Use cargo's dev profile instead of the default release profile.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the computed local version and build command without changing files.",
    )
    args, version_args = parser.parse_known_args(wrapper_and_version_args)
    return args, version_args, cargo_extra_args


def main(argv: list[str]) -> int:
    args, version_args, cargo_extra_args = parse_args(argv)
    repo = repo_root()

    version_cmd = [
        str(repo / "scripts" / "set-local-version.py"),
        "--allow-dirty",
        *version_args,
    ]
    if args.dry_run and "--dry-run" not in version_args:
        version_cmd.append("--dry-run")

    cargo_cmd = ["cargo", "build"]
    if not args.debug:
        cargo_cmd.append("--release")
    cargo_cmd.extend(["-p", "codex-cli", "--bin", "codex"])
    cargo_cmd.extend(cargo_extra_args)

    if args.dry_run:
        run(version_cmd, repo)
        print(f"Build command: (cd codex-rs && {shlex.join(cargo_cmd)})")
        return 0

    snapshots = snapshot_files(repo)
    try:
        run(version_cmd, repo)
        run(cargo_cmd, repo / "codex-rs")
    except subprocess.CalledProcessError as exc:
        return exc.returncode
    finally:
        restore_files(snapshots)

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
