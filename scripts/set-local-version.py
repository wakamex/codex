#!/usr/bin/env python3
"""Set the Rust workspace version to the latest upstream release plus SHA.

This is intended for local/source builds that should report a release-like
version instead of the repository's default 0.0.0 development version.
"""

from __future__ import annotations

import argparse
import functools
import re
import subprocess
import sys
from pathlib import Path


TAG_RE = re.compile(
    r"^rust-v(?P<major>0|[1-9]\d*)\."
    r"(?P<minor>0|[1-9]\d*)\."
    r"(?P<patch>0|[1-9]\d*)"
    r"(?:-(?P<pre>[0-9A-Za-z.-]+))?$"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Patch codex-rs/Cargo.toml so local builds use the latest fetched "
            "rust-v* release version with build metadata for the upstream "
            "commit included in this source tree."
        )
    )
    parser.add_argument(
        "--remote",
        default="upstream",
        help="Git remote that tracks openai/codex (default: upstream).",
    )
    parser.add_argument(
        "--branch",
        default="main",
        help="Remote branch whose tip should be embedded in the version (default: main).",
    )
    parser.add_argument(
        "--source-ref",
        default="HEAD",
        help=(
            "Git ref for the source being built. The appended SHA is the merge-base "
            "of this ref and the upstream branch (default: HEAD)."
        ),
    )
    parser.add_argument(
        "--sha-len",
        type=int,
        default=10,
        help="Number of upstream commit hex characters to append (default: 10).",
    )
    parser.add_argument(
        "--metadata-prefix",
        default="upstream",
        help="SemVer build metadata prefix before the commit SHA (default: upstream).",
    )
    parser.add_argument(
        "--no-fetch",
        action="store_true",
        help="Use already-fetched remote refs and tags.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the computed version without modifying files.",
    )
    parser.add_argument(
        "--skip-lockfile",
        action="store_true",
        help="Do not run cargo metadata to refresh codex-rs/Cargo.lock.",
    )
    parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="Allow codex-rs/Cargo.toml or Cargo.lock to have local changes.",
    )
    return parser.parse_args()


def run(
    args: list[str],
    cwd: Path,
    *,
    stdout: int | None = subprocess.PIPE,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=cwd,
        check=True,
        text=True,
        stdout=stdout,
        stderr=None,
    )


def git(args: list[str], cwd: Path) -> str:
    return run(["git", *args], cwd).stdout.strip()


def remote_release_versions(repo: Path, remote: str) -> list[str]:
    output = git(["ls-remote", "--tags", remote, "refs/tags/rust-v*"], repo)
    versions: list[str] = []
    for line in output.splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        ref = parts[1]
        if ref.endswith("^{}"):
            continue
        tag = ref.removeprefix("refs/tags/")
        match = TAG_RE.match(tag)
        if match is None:
            continue
        versions.append(tag.removeprefix("rust-v"))
    return versions


def latest_version(versions: list[str], context: str) -> str:
    if not versions:
        raise RuntimeError(f"No valid rust-v* release tags found {context}.")
    return max(versions, key=functools.cmp_to_key(compare_versions))


def fetch_version_refs(repo: Path, remote: str, branch: str) -> str:
    """Fetch only the refs needed to compute the local version.

    Avoid `git fetch --tags`: this repository also publishes non-Codex release
    tags, and local tag conflicts in those namespaces should not block this
    helper.
    """

    remote_branch_ref = f"refs/heads/{branch}:refs/remotes/{remote}/{branch}"
    run(["git", "fetch", "--no-tags", remote, remote_branch_ref], repo, stdout=None)

    release_version = latest_version(remote_release_versions(repo, remote), f"on {remote}")
    release_tag_ref = f"refs/tags/rust-v{release_version}:refs/tags/rust-v{release_version}"
    run(
        [
            "git",
            "fetch",
            "--no-tags",
            remote,
            release_tag_ref,
        ],
        repo,
        stdout=None,
    )
    return release_version


def repo_root() -> Path:
    return Path(git(["rev-parse", "--show-toplevel"], Path.cwd()))


def prerelease_key(pre: str) -> list[tuple[int, int | str]]:
    key: list[tuple[int, int | str]] = []
    for part in pre.split("."):
        if part.isdigit():
            key.append((0, int(part)))
        else:
            key.append((1, part))
    return key


def compare_prerelease(left: str | None, right: str | None) -> int:
    if left is None and right is None:
        return 0
    if left is None:
        return 1
    if right is None:
        return -1

    left_key = prerelease_key(left)
    right_key = prerelease_key(right)
    for left_part, right_part in zip(left_key, right_key):
        if left_part == right_part:
            continue
        return -1 if left_part < right_part else 1

    if len(left_key) == len(right_key):
        return 0
    return -1 if len(left_key) < len(right_key) else 1


def compare_versions(left: str, right: str) -> int:
    left_match = TAG_RE.match(f"rust-v{left}")
    right_match = TAG_RE.match(f"rust-v{right}")
    if left_match is None or right_match is None:
        raise ValueError("internal error: invalid parsed version")

    for field in ("major", "minor", "patch"):
        left_num = int(left_match.group(field))
        right_num = int(right_match.group(field))
        if left_num != right_num:
            return -1 if left_num < right_num else 1

    return compare_prerelease(left_match.group("pre"), right_match.group("pre"))


def parsed_release_tags(repo: Path) -> list[str]:
    tags = git(["tag", "--list", "rust-v*"], repo).splitlines()
    versions: list[str] = []
    for tag in tags:
        match = TAG_RE.match(tag)
        if match is None:
            continue
        versions.append(tag.removeprefix("rust-v"))
    return versions


def latest_release_version(repo: Path) -> str:
    return latest_version(parsed_release_tags(repo), "locally")


def replace_workspace_version(cargo_toml: Path, new_version: str) -> str:
    lines = cargo_toml.read_text(encoding="utf-8").splitlines(keepends=True)
    in_workspace_package = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped == "[workspace.package]":
            in_workspace_package = True
            continue
        if in_workspace_package and stripped.startswith("["):
            break
        if in_workspace_package:
            line_body = line.rstrip("\r\n")
            line_ending = line[len(line_body) :]
            match = re.match(r"^(\s*version\s*=\s*\")([^\"]+)(\".*)$", line_body)
            if match is None:
                continue
            old_version = match.group(2)
            lines[index] = f"{match.group(1)}{new_version}{match.group(3)}{line_ending}"
            cargo_toml.write_text("".join(lines), encoding="utf-8")
            return old_version

    raise RuntimeError(f"Could not find [workspace.package] version in {cargo_toml}.")


def current_workspace_version(cargo_toml: Path) -> str:
    text = cargo_toml.read_text(encoding="utf-8")
    match = re.search(
        r"(?ms)^\[workspace\.package\]\s*$(?P<body>.*?)(?:^\[|\Z)",
        text,
    )
    if match is None:
        raise RuntimeError(f"Could not find [workspace.package] in {cargo_toml}.")
    version = re.search(r"(?m)^\s*version\s*=\s*\"([^\"]+)\"", match.group("body"))
    if version is None:
        raise RuntimeError(f"Could not find [workspace.package] version in {cargo_toml}.")
    return version.group(1)


def ensure_clean_version_files(repo: Path) -> None:
    status = git(["status", "--porcelain", "--", "codex-rs/Cargo.toml", "codex-rs/Cargo.lock"], repo)
    if not status:
        return
    raise RuntimeError(
        "codex-rs/Cargo.toml or codex-rs/Cargo.lock already has local changes. "
        "Commit/stash them first, or rerun with --allow-dirty.\n"
        f"{status}"
    )


def main() -> int:
    args = parse_args()
    if args.sha_len < 4:
        raise RuntimeError("--sha-len must be at least 4.")
    if not re.fullmatch(r"[0-9A-Za-z-]+", args.metadata_prefix):
        raise RuntimeError("--metadata-prefix must contain only letters, numbers, or hyphens.")

    repo = repo_root()
    if not args.no_fetch:
        print(f"Fetching {args.remote}/{args.branch} and latest rust-v* release tag...")
        release_version = fetch_version_refs(repo, args.remote, args.branch)
    else:
        release_version = latest_release_version(repo)

    upstream_ref = f"refs/remotes/{args.remote}/{args.branch}"
    try:
        git(["rev-parse", "--verify", "--quiet", upstream_ref], repo)
    except subprocess.CalledProcessError as exc:
        raise RuntimeError(f"Could not resolve {args.remote}/{args.branch}.") from exc

    source_ref = git(["rev-parse", "--verify", "--quiet", args.source_ref], repo)
    upstream_base = git(["merge-base", source_ref, upstream_ref], repo)
    upstream_base_short = git(["rev-parse", f"--short={args.sha_len}", upstream_base], repo)
    target_version = f"{release_version}+{args.metadata_prefix}.{upstream_base_short}"

    cargo_toml = repo / "codex-rs" / "Cargo.toml"
    current_version = current_workspace_version(cargo_toml)
    print(f"Current workspace version: {current_version}")
    print(f"Latest upstream release:  {release_version}")
    print(f"Source ref:               {args.source_ref} {source_ref[:args.sha_len]}")
    print(f"Upstream branch:          {args.remote}/{args.branch}")
    print(f"Included upstream commit: {upstream_base_short}")
    print(f"Target local version:     {target_version}")

    if args.dry_run:
        return 0

    if not args.allow_dirty:
        ensure_clean_version_files(repo)

    old_version = replace_workspace_version(cargo_toml, target_version)
    print(f"Updated codex-rs/Cargo.toml: {old_version} -> {target_version}")

    if not args.skip_lockfile:
        print("Refreshing codex-rs/Cargo.lock with cargo metadata...")
        run(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(cargo_toml),
                "--format-version",
                "1",
                "--no-deps",
            ],
            repo,
            stdout=subprocess.DEVNULL,
        )

    print("Done. Build from codex-rs with cargo build -p codex-cli.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
