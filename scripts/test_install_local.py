#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import call
from unittest.mock import patch


SCRIPT_PATH = Path(__file__).with_name("install-local.py")
SPEC = importlib.util.spec_from_file_location("install_local", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"Could not load {SCRIPT_PATH}")
INSTALL_LOCAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL_LOCAL)


class InstallBinariesTest(unittest.TestCase):
    def test_validates_all_sources_before_installing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            release_dir = root / "release"
            install_dir = root / "bin"
            release_dir.mkdir()
            install_dir.mkdir()
            (release_dir / "codex").touch()

            with patch.object(INSTALL_LOCAL.subprocess, "run") as run:
                with self.assertRaisesRegex(RuntimeError, "Missing release binaries"):
                    INSTALL_LOCAL.install_binaries(release_dir, install_dir, ["sudo"])

            run.assert_not_called()

    def test_backs_up_existing_binary_and_installs_both(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            release_dir = root / "release"
            install_dir = root / "bin"
            release_dir.mkdir()
            install_dir.mkdir()
            for binary in INSTALL_LOCAL.BINARIES:
                (release_dir / binary).touch()
            (install_dir / "codex").touch()

            with patch.object(INSTALL_LOCAL.subprocess, "run") as run:
                backups = INSTALL_LOCAL.install_binaries(
                    release_dir, install_dir, ["sudo"]
                )

            self.assertEqual(backups, [install_dir / "codex_bkup"])
            self.assertEqual(
                run.call_args_list,
                [
                    call(
                        [
                            "sudo",
                            "install",
                            "-m",
                            "0755",
                            str(install_dir / "codex"),
                            str(install_dir / "codex_bkup"),
                        ],
                        check=True,
                    ),
                    call(
                        [
                            "sudo",
                            "install",
                            "-m",
                            "0755",
                            str(release_dir / "codex"),
                            str(install_dir / "codex"),
                        ],
                        check=True,
                    ),
                    call(
                        [
                            "sudo",
                            "install",
                            "-m",
                            "0755",
                            str(release_dir / "codex-code-mode-host"),
                            str(install_dir / "codex-code-mode-host"),
                        ],
                        check=True,
                    ),
                ],
            )


if __name__ == "__main__":
    unittest.main()
