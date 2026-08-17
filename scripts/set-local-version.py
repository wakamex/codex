#!/usr/bin/env python3
"""Set the Rust workspace version from the included upstream and fork SHAs.

This is intended for local/source builds that should report a release-like
version instead of the repository's default 0.0.0 development version.
"""

import argparse
import functools
import os
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
VERSION_STAMP_COMMIT_SUBJECT = "Stamp workspace with local version"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Patch codex-rs/Cargo.toml so local builds use the latest local "
            "rust-v* release plus the upstream and fork commits represented "
            "in this source tree."
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
        help="Remote branch used to find the included upstream commit (default: main).",
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
        "--dry-run",
        action="store_true",
        help="Print the computed version without modifying files.",
    )
    parser.add_argument(
        "--skip-lockfile",
        action="store_true",
        help="Do not run cargo metadata to refresh codex-rs/Cargo.lock.",
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


def latest_version(versions: list[str], context: str) -> str:
    if not versions:
        raise RuntimeError(f"No valid rust-v* release tags found {context}.")
    return max(versions, key=functools.cmp_to_key(compare_versions))


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


def release_tag_is_represented_in_base(
    tagged_commit: str, tagged_parents: list[str], upstream_commits: set[str]
) -> bool:
    return tagged_commit in upstream_commits or any(
        parent in upstream_commits for parent in tagged_parents
    )


def parsed_release_tags(repo: Path, upstream_base: str) -> list[str]:
    tag_refs = git(
        [
            "for-each-ref",
            "--format=%(refname:short)%00%(*objectname)%00%(*parent)%00%(objectname)%00%(parent)",
            "refs/tags/rust-v*",
        ],
        repo,
    ).splitlines()
    upstream_commits = set(git(["rev-list", upstream_base], repo).splitlines())
    versions: list[str] = []
    for tag_ref in tag_refs:
        fields = tag_ref.split("\0")
        if len(fields) != 5:
            continue
        tag, peeled_commit, peeled_parents, object_name, object_parents = fields
        match = TAG_RE.match(tag)
        tagged_commit = peeled_commit or object_name
        tagged_parents = (peeled_parents or object_parents).split()
        if match is None or not release_tag_is_represented_in_base(
            tagged_commit, tagged_parents, upstream_commits
        ):
            continue
        versions.append(tag.removeprefix("rust-v"))
    return versions


def latest_release_version(repo: Path, upstream_base: str) -> str:
    return latest_version(
        parsed_release_tags(repo, upstream_base),
        f"represented in {upstream_base}; run 'just rebase-upstream' to refresh refs and tags",
    )


def fork_source_commit(repo: Path, source_commit: str) -> str:
    subject = git(["show", "-s", "--format=%s", source_commit], repo)
    if subject != VERSION_STAMP_COMMIT_SUBJECT:
        return source_commit

    parents = git(["show", "-s", "--format=%P", source_commit], repo).split()
    if not parents:
        raise RuntimeError("Version stamp commit has no parent source commit.")
    return parents[0]


def local_version(
    release_version: str,
    metadata_prefix: str,
    upstream_base_short: str,
    fork_source_short: str,
) -> str:
    return (
        f"{release_version}+{metadata_prefix}.{upstream_base_short}"
        f".fork.{fork_source_short}"
    )


def replace_workspace_version(cargo_toml: Path, new_version: str) -> str:
    lines = cargo_toml.read_bytes().decode("utf-8").splitlines(keepends=True)
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
            cargo_toml.write_bytes("".join(lines).encode("utf-8"))
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
        raise RuntimeError(
            f"Could not find [workspace.package] version in {cargo_toml}."
        )
    return version.group(1)


def ensure_clean_worktree(repo: Path) -> None:
    status = git(["status", "--porcelain", "--untracked-files=all"], repo)
    if not status:
        return
    raise RuntimeError(
        "Refusing to stamp a dirty worktree. Commit, stash, or remove every "
        "tracked and untracked change first.\n"
        f"{status}"
    )


def update_workspace_version(
    repo: Path,
    cargo_toml: Path,
    target_version: str,
    *,
    refresh_lockfile: bool,
) -> None:
    version_files = [cargo_toml]
    if refresh_lockfile:
        version_files.append(repo / "codex-rs" / "Cargo.lock")
    snapshots = [
        (path, path.read_bytes(), path.stat().st_mode) for path in version_files
    ]

    try:
        old_version = replace_workspace_version(cargo_toml, target_version)
        print(f"Updated codex-rs/Cargo.toml: {old_version} -> {target_version}")

        if refresh_lockfile:
            print("Refreshing codex-rs/Cargo.lock with cargo metadata...")
            run(
                [
                    "cargo",
                    "metadata",
                    "--manifest-path",
                    str(cargo_toml),
                    "--format-version",
                    "1",
                ],
                repo,
                stdout=subprocess.DEVNULL,
            )
    except Exception:
        for path, contents, mode in snapshots:
            path.write_bytes(contents)
            os.chmod(path, mode)
        print("Restored version files after the update failed.", file=sys.stderr)
        raise


def main() -> int:
    args = parse_args()
    if args.sha_len < 4:
        raise RuntimeError("--sha-len must be at least 4.")
    if not re.fullmatch(r"[0-9A-Za-z-]+", args.metadata_prefix):
        raise RuntimeError(
            "--metadata-prefix must contain only letters, numbers, or hyphens."
        )

    repo = repo_root()
    upstream_ref = f"refs/remotes/{args.remote}/{args.branch}"
    try:
        git(["rev-parse", "--verify", "--quiet", upstream_ref], repo)
    except subprocess.CalledProcessError as exc:
        raise RuntimeError(f"Could not resolve {args.remote}/{args.branch}.") from exc

    source_ref = git(["rev-parse", "--verify", "--quiet", args.source_ref], repo)
    upstream_base = git(["merge-base", source_ref, upstream_ref], repo)
    fork_source = fork_source_commit(repo, source_ref)
    release_version = latest_release_version(repo, upstream_base)
    upstream_base_short = git(
        ["rev-parse", f"--short={args.sha_len}", upstream_base], repo
    )
    fork_source_short = git(["rev-parse", f"--short={args.sha_len}", fork_source], repo)
    target_version = local_version(
        release_version,
        args.metadata_prefix,
        upstream_base_short,
        fork_source_short,
    )

    cargo_toml = repo / "codex-rs" / "Cargo.toml"
    current_version = current_workspace_version(cargo_toml)
    print(f"Current workspace version: {current_version}")
    print(f"Base upstream release:    {release_version}")
    print(f"Source ref:               {args.source_ref} {source_ref[: args.sha_len]}")
    print(f"Upstream branch:          {args.remote}/{args.branch}")
    print(f"Included upstream commit: {upstream_base_short}")
    print(f"Included fork commit:     {fork_source_short}")
    print(f"Target local version:     {target_version}")

    if args.dry_run:
        return 0

    ensure_clean_worktree(repo)

    update_workspace_version(
        repo,
        cargo_toml,
        target_version,
        refresh_lockfile=not args.skip_lockfile,
    )

    print("Done. Build from codex-rs with cargo build -p codex-cli.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
