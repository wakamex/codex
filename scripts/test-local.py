#!/usr/bin/env python3
"""Run the workspace tests in an environment isolated from the host user."""

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import Mapping

from codex_package.targets import TARGET_SPECS
from codex_package.v8 import resolve_codex_v8_cargo_env

_BUILD_LOCAL_PATH = Path(__file__).with_name("build-local.py")
_SPEC = importlib.util.spec_from_file_location("build_local", _BUILD_LOCAL_PATH)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError(f"Could not load {_BUILD_LOCAL_PATH}")
BUILD_LOCAL = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(BUILD_LOCAL)

RUST_MIN_STACK = "8388608"
VOICE_HOST_PACKAGE = "codex-voice-host"
GSTREAMER_REQUIREMENT = "gstreamer-1.0 >= 1.28"
DEFAULT_SCRATCH_ROOT = Path("/var/tmp/codex-test-local")


def isolated_env(
    base_env: Mapping[str, str], scratch: Path, v8_env: Mapping[str, str]
) -> dict[str, str]:
    """Point HOME and TMPDIR at scratch while keeping the real Rust toolchain homes."""
    real_home = Path(base_env.get("HOME", str(Path.home())))
    env = {**base_env, **v8_env}
    env.setdefault("CARGO_HOME", str(real_home / ".cargo"))
    env.setdefault("RUSTUP_HOME", str(real_home / ".rustup"))
    env["HOME"] = str(scratch / "home")
    env["TMPDIR"] = str(scratch / "tmp")
    env["RUST_MIN_STACK"] = RUST_MIN_STACK
    env["NEXTEST_PROFILE"] = "local"
    return env


def scratch_root(env: Mapping[str, str]) -> Path:
    """Keep test scratch outside git checkouts, dot directories and small tmpfs mounts."""
    configured = env.get("CODEX_TEST_LOCAL_DIR")
    return Path(configured) if configured else DEFAULT_SCRATCH_ROOT


def default_nextest_args(has_gstreamer: bool) -> list[str]:
    args = ["--workspace"]
    if not has_gstreamer:
        args += ["--exclude", VOICE_HOST_PACKAGE]
    return args


def gstreamer_available() -> bool:
    if shutil.which("pkg-config") is None:
        return False
    result = subprocess.run(
        ["pkg-config", "--exists", GSTREAMER_REQUIREMENT], check=False
    )
    return result.returncode == 0


def main() -> int:
    codex_rs = BUILD_LOCAL.repo_root() / "codex-rs"
    spec = TARGET_SPECS[BUILD_LOCAL.rust_host_target()]
    v8_env = resolve_codex_v8_cargo_env(spec)
    scratch = scratch_root(os.environ)
    shutil.rmtree(scratch / "home", ignore_errors=True)
    for name in ("home", "tmp"):
        (scratch / name).mkdir(parents=True, exist_ok=True)
    env = isolated_env(os.environ, scratch, v8_env)

    args = sys.argv[1:]
    if not args:
        has_gstreamer = gstreamer_available()
        if not has_gstreamer:
            print(
                f"{GSTREAMER_REQUIREMENT} not found; excluding {VOICE_HOST_PACKAGE}.",
                flush=True,
            )
        args = default_nextest_args(has_gstreamer)
    command = ["cargo", "nextest", "run", "--no-fail-fast", *args]
    return subprocess.run(command, cwd=codex_rs, env=env, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())
