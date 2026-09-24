#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT_PATH = Path(__file__).with_name("install-local.py")
SPEC = importlib.util.spec_from_file_location("install_local", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"Could not load {SCRIPT_PATH}")
INSTALL_LOCAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL_LOCAL)


class ReleaseBinariesTest(unittest.TestCase):
    def test_reports_every_missing_binary(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            release_dir = Path(temporary_directory)
            (release_dir / "codex").touch()

            with self.assertRaisesRegex(RuntimeError, "codex-code-mode-host"):
                INSTALL_LOCAL.release_binaries(release_dir)


class InstallPackageTest(unittest.TestCase):
    def test_backs_up_existing_package_and_links_entrypoint(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            staged = root / "staged"
            package_dir = root / "lib" / "codex"
            install_dir = root / "bin"
            package_dir.mkdir(parents=True)
            install_dir.mkdir()

            with patch.object(INSTALL_LOCAL.subprocess, "run") as run:
                backup = INSTALL_LOCAL.install_package(
                    staged, package_dir, install_dir, ["sudo"]
                )

            incoming = root / "lib" / "codex_new"
            self.assertEqual(backup, root / "lib" / "codex_bkup")
            self.assertEqual(
                [entry.args[0] for entry in run.call_args_list],
                [
                    ["sudo", "rm", "-rf", str(incoming)],
                    ["sudo", "cp", "-a", str(staged), str(incoming)],
                    ["sudo", "rm", "-rf", str(backup)],
                    ["sudo", "mv", str(package_dir), str(backup)],
                    ["sudo", "mv", str(incoming), str(package_dir)],
                    [
                        "sudo",
                        "ln",
                        "-sfn",
                        str(package_dir / "bin" / "codex"),
                        str(install_dir / "codex"),
                    ],
                ],
            )

    def test_first_install_has_no_backup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            install_dir = root / "bin"
            install_dir.mkdir()

            with patch.object(INSTALL_LOCAL.subprocess, "run") as run:
                backup = INSTALL_LOCAL.install_package(
                    root / "staged", root / "codex", install_dir, []
                )

            self.assertIsNone(backup)
            self.assertEqual(
                [entry.args[0][:2] for entry in run.call_args_list],
                [
                    ["rm", "-rf"],
                    ["cp", "-a"],
                    ["mv", str(root / "codex_new")],
                    ["ln", "-sfn"],
                ],
            )


class PruneLocalReleasesTest(unittest.TestCase):
    def test_keeps_only_the_selected_local_release(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            daemon_root = Path(temporary_directory)
            releases = daemon_root / "releases"
            for name in ("local-old", "local-current", "0.150.0"):
                (releases / name).mkdir(parents=True)
            (daemon_root / "current").symlink_to(releases / "local-current")

            INSTALL_LOCAL.prune_local_releases(daemon_root)

            self.assertEqual(
                sorted(path.name for path in releases.iterdir()),
                ["0.150.0", "local-current"],
            )


if __name__ == "__main__":
    unittest.main()
