#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import unittest


SCRIPT_PATH = Path(__file__).with_name("test-local.py")
SPEC = importlib.util.spec_from_file_location("test_local", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"Could not load {SCRIPT_PATH}")
TEST_LOCAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TEST_LOCAL)


class IsolatedEnvTest(unittest.TestCase):
    def test_isolates_home_and_tmpdir_but_keeps_toolchain_homes(self) -> None:
        env = TEST_LOCAL.isolated_env(
            {"HOME": "/home/user", "PATH": "/usr/bin"},
            Path("/target/test-local"),
            {"RUSTY_V8_ARCHIVE": "/cache/v8.a.gz"},
        )

        self.assertEqual(
            env,
            {
                "HOME": "/target/test-local/home",
                "TMPDIR": "/target/test-local/tmp",
                "PATH": "/usr/bin",
                "CARGO_HOME": "/home/user/.cargo",
                "RUSTUP_HOME": "/home/user/.rustup",
                "RUSTY_V8_ARCHIVE": "/cache/v8.a.gz",
                "RUST_MIN_STACK": "8388608",
                "NEXTEST_PROFILE": "local",
            },
        )

    def test_preserves_configured_toolchain_homes(self) -> None:
        env = TEST_LOCAL.isolated_env(
            {"HOME": "/home/user", "CARGO_HOME": "/opt/cargo"},
            Path("/scratch"),
            {},
        )

        self.assertEqual(env["CARGO_HOME"], "/opt/cargo")
        self.assertEqual(env["RUSTUP_HOME"], "/home/user/.rustup")


class ScratchRootTest(unittest.TestCase):
    def test_defaults_to_var_tmp(self) -> None:
        self.assertEqual(
            TEST_LOCAL.scratch_root({"HOME": "/home/user"}),
            Path("/var/tmp/codex-test-local"),
        )

    def test_explicit_directory_overrides_default(self) -> None:
        self.assertEqual(
            TEST_LOCAL.scratch_root({"CODEX_TEST_LOCAL_DIR": "/scratch"}),
            Path("/scratch"),
        )


class DefaultNextestArgsTest(unittest.TestCase):
    def test_excludes_voice_host_without_gstreamer(self) -> None:
        self.assertEqual(
            TEST_LOCAL.default_nextest_args(has_gstreamer=False),
            ["--workspace", "--exclude", "codex-voice-host"],
        )

    def test_runs_whole_workspace_with_gstreamer(self) -> None:
        self.assertEqual(
            TEST_LOCAL.default_nextest_args(has_gstreamer=True), ["--workspace"]
        )


if __name__ == "__main__":
    unittest.main()
