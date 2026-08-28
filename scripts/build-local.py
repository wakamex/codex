#!/usr/bin/env python3
"""Build the local Codex CLI and its code-mode host sidecar."""

import os
from pathlib import Path
import shlex
import subprocess
import sys

from codex_package.targets import TARGET_SPECS
from codex_package.v8 import resolve_codex_v8_cargo_env


def repo_root() -> Path:
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return Path(result.stdout.strip())


def rust_host_target() -> str:
    result = subprocess.run(
        ["rustc", "-vV"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    host = next(
        (
            line.removeprefix("host: ")
            for line in result.stdout.splitlines()
            if line.startswith("host: ")
        ),
        None,
    )
    if host is None:
        raise RuntimeError("Could not determine the host target from `rustc -vV`.")
    if host not in TARGET_SPECS:
        supported = ", ".join(sorted(TARGET_SPECS))
        raise RuntimeError(
            f"Unsupported host target {host}. Supported targets: {supported}"
        )
    return host


def target_dir(codex_rs: Path) -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if configured is None:
        return codex_rs / "target"
    path = Path(configured)
    return path if path.is_absolute() else codex_rs / path


def ensure_clean_worktree(repo: Path) -> None:
    result = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=repo,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    status = result.stdout.strip()
    if status:
        raise RuntimeError(
            "Refusing to build a dirty worktree. Commit, stash, or remove every "
            f"tracked and untracked change first.\n{status}"
        )


def verify_local_version(repo: Path) -> None:
    subprocess.run(
        [sys.executable, repo / "scripts" / "set-local-version.py", "--check"],
        cwd=repo,
        check=True,
    )


def main() -> int:
    repo = repo_root()
    ensure_clean_worktree(repo)
    verify_local_version(repo)
    codex_rs = repo / "codex-rs"
    spec = TARGET_SPECS[rust_host_target()]
    env = {**os.environ, **resolve_codex_v8_cargo_env(spec)}
    command = [
        "cargo",
        "build",
        "--release",
        "-p",
        "codex-cli",
        "--bin",
        "codex",
        "-p",
        "codex-code-mode-host",
        "--bin",
        "codex-code-mode-host",
    ]

    print(f"+ {shlex.join(command)}", flush=True)
    subprocess.run(command, cwd=codex_rs, check=True, env=env)

    output_dir = target_dir(codex_rs) / "release"
    outputs = [
        output_dir / f"codex{spec.exe_suffix}",
        output_dir / f"codex-code-mode-host{spec.exe_suffix}",
    ]
    missing = [path for path in outputs if not path.is_file()]
    if missing:
        paths = ", ".join(str(path) for path in missing)
        raise RuntimeError(
            f"Cargo completed without producing expected binaries: {paths}"
        )

    print("Built:")
    for path in outputs:
        print(path)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
