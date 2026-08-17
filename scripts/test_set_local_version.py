#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPT_PATH = Path(__file__).with_name("set-local-version.py")
SPEC = importlib.util.spec_from_file_location("set_local_version", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"Could not load {SCRIPT_PATH}")
SET_LOCAL_VERSION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SET_LOCAL_VERSION)


class LatestReleaseVersionTest(unittest.TestCase):
    def test_uses_latest_release_represented_in_upstream_base(self) -> None:
        repo = Path("/repo")
        tag_refs = "\n".join(
            [
                "rust-v0.146.0\0release-146\0base-146\0tag-146\0",
                "rust-v0.147.0-alpha.9\0release-147-9\0base-147\0tag-147-9\0",
                "rust-v0.147.0-alpha.11\0release-147-11\0base-147\0tag-147-11\0",
                "rust-v0.148.0-alpha.17\0release-148\0after-base\0tag-148\0",
                "rust-vinvalid\0invalid\0base-147\0tag-invalid\0",
            ]
        )
        with patch.object(
            SET_LOCAL_VERSION,
            "git",
            side_effect=[tag_refs, "upstream-base\nbase-146\nbase-147"],
        ):
            version = SET_LOCAL_VERSION.latest_release_version(repo, "upstream-base")

        self.assertEqual(version, "0.147.0-alpha.11")

    def test_accepts_release_commit_based_directly_on_upstream_base(self) -> None:
        represented = SET_LOCAL_VERSION.release_tag_is_represented_in_base(
            "tagged", ["parent"], {"upstream-base", "parent"}
        )

        self.assertTrue(represented)

    def test_rejects_release_commit_based_after_upstream_base(self) -> None:
        represented = SET_LOCAL_VERSION.release_tag_is_represented_in_base(
            "tagged", ["parent"], {"upstream-base", "older"}
        )

        self.assertFalse(represented)


class WorkspaceVersionUpdateTest(unittest.TestCase):
    def test_replace_workspace_version_preserves_crlf(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            cargo_toml = Path(temp_dir) / "Cargo.toml"
            cargo_toml.write_bytes(
                b'[workspace]\r\n\r\n[workspace.package]\r\nversion = "0.0.0"\r\n'
            )

            old_version = SET_LOCAL_VERSION.replace_workspace_version(
                cargo_toml, "1.2.3+upstream.abcdef1234"
            )

            self.assertEqual(old_version, "0.0.0")
            self.assertEqual(
                cargo_toml.read_bytes(),
                b'[workspace]\r\n\r\n[workspace.package]\r\nversion = "1.2.3+upstream.abcdef1234"\r\n',
            )

    def test_update_workspace_version_without_lockfile_refresh(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            codex_rs = repo / "codex-rs"
            codex_rs.mkdir()
            cargo_toml = codex_rs / "Cargo.toml"
            cargo_toml.write_bytes(b'[workspace.package]\nversion = "0.0.0"\n')

            SET_LOCAL_VERSION.update_workspace_version(
                repo,
                cargo_toml,
                "1.2.3+upstream.abcdef1234",
                refresh_lockfile=False,
            )

            self.assertEqual(
                cargo_toml.read_bytes(),
                b'[workspace.package]\nversion = "1.2.3+upstream.abcdef1234"\n',
            )

    def test_update_workspace_version_restores_files_when_metadata_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            codex_rs = repo / "codex-rs"
            codex_rs.mkdir()
            cargo_toml = codex_rs / "Cargo.toml"
            cargo_lock = codex_rs / "Cargo.lock"
            cargo_toml.write_bytes(b'[workspace.package]\nversion = "0.0.0"\n')
            cargo_lock.write_bytes(b"original lockfile\n")
            original_files = {
                cargo_toml: cargo_toml.read_bytes(),
                cargo_lock: cargo_lock.read_bytes(),
            }

            def fail_metadata(*_args, **_kwargs):
                cargo_lock.write_bytes(b"partial lockfile\n")
                raise subprocess.CalledProcessError(1, ["cargo", "metadata"])

            with patch.object(SET_LOCAL_VERSION, "run", side_effect=fail_metadata):
                with self.assertRaises(subprocess.CalledProcessError):
                    SET_LOCAL_VERSION.update_workspace_version(
                        repo,
                        cargo_toml,
                        "1.2.3+upstream.abcdef1234",
                        refresh_lockfile=True,
                    )

            self.assertEqual(
                {path: path.read_bytes() for path in original_files}, original_files
            )


if __name__ == "__main__":
    unittest.main()
