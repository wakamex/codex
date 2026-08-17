#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import subprocess
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


if __name__ == "__main__":
    unittest.main()
