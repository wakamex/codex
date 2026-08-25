#!/usr/bin/env python3
"""Install locally built Codex binaries with rollback copies."""

import argparse
import os
from pathlib import Path
import shlex
import subprocess
import sys


BINARIES = ("codex", "codex-code-mode-host")


def repo_root() -> Path:
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return Path(result.stdout.strip())


def target_dir(codex_rs: Path) -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    if configured is None:
        return codex_rs / "target"
    path = Path(configured)
    return path if path.is_absolute() else codex_rs / path


def run(command: list[str | Path]) -> None:
    printable = [str(argument) for argument in command]
    print(f"+ {shlex.join(printable)}", flush=True)
    subprocess.run(printable, check=True)


def install_binaries(
    release_dir: Path,
    install_dir: Path,
    privilege_prefix: list[str],
) -> list[Path]:
    sources = [release_dir / binary for binary in BINARIES]
    missing = [source for source in sources if not source.is_file()]
    if missing:
        paths = ", ".join(str(path) for path in missing)
        raise RuntimeError(
            f"Missing release binaries: {paths}. Run `just build-local`."
        )
    if not install_dir.is_dir():
        raise RuntimeError(f"Install directory does not exist: {install_dir}")

    backups = []
    for source in sources:
        target = install_dir / source.name
        if target.exists():
            backup = target.with_name(f"{target.name}_bkup")
            run([*privilege_prefix, "install", "-m", "0755", target, backup])
            backups.append(backup)
        run([*privilege_prefix, "install", "-m", "0755", source, target])
    return backups


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Install locally built Codex release binaries."
    )
    parser.add_argument(
        "--install-dir",
        type=Path,
        default=Path("/usr/local/bin"),
        help="installation directory (default: /usr/local/bin)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    codex_rs = repo_root() / "codex-rs"
    release_dir = target_dir(codex_rs) / "release"
    install_dir_is_writable = os.access(args.install_dir, os.W_OK)
    privilege_prefix = [] if os.geteuid() == 0 or install_dir_is_writable else ["sudo"]
    backups = install_binaries(release_dir, args.install_dir, privilege_prefix)

    installed_codex = args.install_dir / "codex"
    print("Installed CLI version:")
    run([installed_codex, "--version"])

    if backups:
        print("Rollback:")
        for backup in backups:
            target = backup.with_name(backup.name.removesuffix("_bkup"))
            command = [*privilege_prefix, "install", "-m", "0755", backup, target]
            print(shlex.join(str(argument) for argument in command))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
