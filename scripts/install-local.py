#!/usr/bin/env python3
"""Install locally built Codex binaries as a package with a rollback copy."""

import argparse
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile


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


def rust_host_target() -> str:
    result = subprocess.run(
        ["rustc", "-vV"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    for line in result.stdout.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    raise RuntimeError("Could not determine the host target from `rustc -vV`.")


def run(command: list[str | Path], env: dict[str, str] | None = None) -> None:
    printable = [str(argument) for argument in command]
    print(f"+ {shlex.join(printable)}", flush=True)
    subprocess.run(printable, check=True, env=env)


def release_binaries(release_dir: Path) -> list[Path]:
    sources = [release_dir / binary for binary in BINARIES]
    missing = [source for source in sources if not source.is_file()]
    if missing:
        paths = ", ".join(str(path) for path in missing)
        raise RuntimeError(
            f"Missing release binaries: {paths}. Run `just build-local`."
        )
    return sources


def assemble_package(repo: Path, release_dir: Path, package_dir: Path) -> None:
    codex, code_mode_host = release_binaries(release_dir)
    version = subprocess.run(
        [codex, "--version"], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.split()[-1]
    run(
        [
            sys.executable,
            repo / "scripts" / "build_codex_package.py",
            "--target",
            rust_host_target(),
            "--cargo-profile",
            "release",
            "--entrypoint-bin",
            codex,
            "--code-mode-host-bin",
            code_mode_host,
            "--package-version",
            version,
            "--package-dir",
            package_dir,
            "--force",
        ],
        env={**os.environ, "CODEX_REPO_ROOT": str(repo)},
    )


def install_package(
    staged: Path,
    package_dir: Path,
    install_dir: Path,
    privilege_prefix: list[str],
) -> Path | None:
    """Replace package_dir with staged and link install_dir/codex into it."""
    if not install_dir.is_dir():
        raise RuntimeError(f"Install directory does not exist: {install_dir}")
    # Copy beside the destination first so a failed copy leaves the old package.
    incoming = package_dir.with_name(f"{package_dir.name}_new")
    run([*privilege_prefix, "rm", "-rf", incoming])
    run([*privilege_prefix, "cp", "-a", staged, incoming])
    backup = package_dir.with_name(f"{package_dir.name}_bkup")
    if package_dir.exists():
        run([*privilege_prefix, "rm", "-rf", backup])
        run([*privilege_prefix, "mv", package_dir, backup])
    else:
        backup = None
    run([*privilege_prefix, "mv", incoming, package_dir])
    run(
        [
            *privilege_prefix,
            "ln",
            "-sfn",
            package_dir / "bin" / "codex",
            install_dir / "codex",
        ]
    )
    return backup


def update_daemon(codex: Path) -> None:
    """Point an existing app-server daemon at the newly installed package."""
    codex_home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    if not (codex_home / "packages" / "app-server-daemon" / "current").exists():
        print("No app-server daemon package is selected; skipping daemon update.")
        return
    run([codex, "app-server", "daemon", "update", "--from-cli", "--yes"])


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Install locally built Codex release binaries as a package."
    )
    parser.add_argument(
        "--install-dir",
        type=Path,
        default=Path("/usr/local/bin"),
        help="directory for the codex symlink (default: /usr/local/bin)",
    )
    parser.add_argument(
        "--package-dir",
        type=Path,
        default=Path("/usr/local/lib/codex"),
        help="package installation directory (default: /usr/local/lib/codex)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = repo_root()
    cargo_target = target_dir(repo / "codex-rs")
    release_dir = cargo_target / "release"
    writable = all(
        os.access(path, os.W_OK) for path in (args.install_dir, args.package_dir.parent)
    )
    privilege_prefix = [] if os.geteuid() == 0 or writable else ["sudo"]

    # The package is large; keep staging off a potentially RAM-backed /tmp.
    with tempfile.TemporaryDirectory(dir=cargo_target) as temporary_directory:
        staged = Path(temporary_directory) / "codex"
        assemble_package(repo, release_dir, staged)
        backup = install_package(
            staged, args.package_dir, args.install_dir, privilege_prefix
        )

    installed_codex = args.install_dir / "codex"
    print("Installed CLI version:")
    run([installed_codex, "--version"])
    update_daemon(installed_codex)

    if backup:
        print("Rollback:")
        restore = [
            [*privilege_prefix, "rm", "-rf", args.package_dir],
            [*privilege_prefix, "mv", backup, args.package_dir],
            [installed_codex, "app-server", "daemon", "update", "--from-cli", "--yes"],
        ]
        for command in restore:
            print(shlex.join(str(argument) for argument in command))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
