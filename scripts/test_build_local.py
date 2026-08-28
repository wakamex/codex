#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPT_PATH = Path(__file__).with_name("build-local.py")
SPEC = importlib.util.spec_from_file_location("build_local", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"Could not load {SCRIPT_PATH}")
BUILD_LOCAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD_LOCAL)


class RustHostTargetTest(unittest.TestCase):
    def test_reads_supported_host_from_rustc(self) -> None:
        rustc_output = subprocess.CompletedProcess(
            ["rustc", "-vV"],
            0,
            stdout="rustc 1.95.0\nhost: x86_64-unknown-linux-gnu\n",
        )
        with patch.object(BUILD_LOCAL.subprocess, "run", return_value=rustc_output):
            target = BUILD_LOCAL.rust_host_target()

        self.assertEqual(target, "x86_64-unknown-linux-gnu")

    def test_rejects_unsupported_host(self) -> None:
        rustc_output = subprocess.CompletedProcess(
            ["rustc", "-vV"],
            0,
            stdout="rustc 1.95.0\nhost: powerpc64le-unknown-linux-gnu\n",
        )
        with patch.object(BUILD_LOCAL.subprocess, "run", return_value=rustc_output):
            with self.assertRaisesRegex(RuntimeError, "Unsupported host target"):
                BUILD_LOCAL.rust_host_target()


class WorktreeValidationTest(unittest.TestCase):
    def test_rejects_dirty_worktree(self) -> None:
        result = subprocess.CompletedProcess(
            ["git", "status"],
            0,
            stdout=" M scripts/build-local.py\n",
        )
        with patch.object(BUILD_LOCAL.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "dirty worktree"):
                BUILD_LOCAL.ensure_clean_worktree(Path("/repo"))

    def test_temporary_local_version_restores_version_files(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            codex_rs = repo / "codex-rs"
            codex_rs.mkdir()
            cargo_toml = codex_rs / "Cargo.toml"
            cargo_lock = codex_rs / "Cargo.lock"
            cargo_toml.write_text("original manifest\n")
            cargo_lock.write_text("original lockfile\n")
            original = {
                cargo_toml: cargo_toml.read_bytes(),
                cargo_lock: cargo_lock.read_bytes(),
            }

            def stamp(*_args, **_kwargs):
                cargo_toml.write_text("stamped manifest\n")
                cargo_lock.write_text("stamped lockfile\n")
                return subprocess.CompletedProcess([], 0)

            with patch.object(BUILD_LOCAL.subprocess, "run", side_effect=stamp):
                with BUILD_LOCAL.temporary_local_version(repo):
                    self.assertEqual(
                        {
                            cargo_toml: cargo_toml.read_text(),
                            cargo_lock: cargo_lock.read_text(),
                        },
                        {
                            cargo_toml: "stamped manifest\n",
                            cargo_lock: "stamped lockfile\n",
                        },
                    )

            self.assertEqual(
                {path: path.read_bytes() for path in original},
                original,
            )

    def test_temporary_local_version_restores_after_stamp_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            codex_rs = repo / "codex-rs"
            codex_rs.mkdir()
            cargo_toml = codex_rs / "Cargo.toml"
            cargo_lock = codex_rs / "Cargo.lock"
            cargo_toml.write_text("original manifest\n")
            cargo_lock.write_text("original lockfile\n")
            original = {
                cargo_toml: cargo_toml.read_bytes(),
                cargo_lock: cargo_lock.read_bytes(),
            }

            def fail_stamp(*_args, **_kwargs):
                cargo_toml.write_text("partial manifest\n")
                raise subprocess.CalledProcessError(1, ["set-local-version.py"])

            with patch.object(BUILD_LOCAL.subprocess, "run", side_effect=fail_stamp):
                with self.assertRaises(subprocess.CalledProcessError):
                    with BUILD_LOCAL.temporary_local_version(repo):
                        pass

            self.assertEqual(
                {path: path.read_bytes() for path in original},
                original,
            )


if __name__ == "__main__":
    unittest.main()
