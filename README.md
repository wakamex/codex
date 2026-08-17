<p align="center"><strong>Codex CLI</strong> is a coding agent from OpenAI that runs locally on your computer.
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
If you want Codex in your code editor (VS Code, Cursor, Windsurf), <a href="https://developers.openai.com/codex/ide">install in your IDE.</a>
</br>If you want the desktop app experience, run <code>codex app</code> or visit <a href="https://chatgpt.com/codex?app-landing-page=true">the Codex App page</a>.
</br>If you are looking for the <em>cloud-based agent</em> from OpenAI, <strong>Codex Web</strong>, go to <a href="https://chatgpt.com/codex">chatgpt.com/codex</a>.</p>

---

## Fork Additions

The material user-facing additions are TUI loops, completion timestamps, tool-free exec sessions, configurable same-turn failure handlers, and preserved reconnect commands. Repository maintenance helpers are documented separately below.

### TUI Loops

The built-in `/loop` command can run a prompt on an interval or continuously:

```text
/loop 5m check the repo for new changes and act if needed
/loop continuous keep checking until I stop it
/loop status
/loop off
```

Current v1 limits:

- session-local only
- in-memory only
- only one active loop per session
- interval loops only submit when the agent is idle
- `continuous` loops submit immediately when idle, or after the current task when enabled mid-turn, then again at each live turn boundary

### Finished Timestamps

The TUI turn separator always includes the local completion time and date:

```text
Worked for 5m 57s, finished at 14:32 on 17 Aug 2026
```

### Tool-Free Exec

Use `--no-tools` for new or resumed non-interactive sessions without exposing model-visible tools:

```shell
codex exec --no-tools "Answer using only your existing context"
codex exec resume --last --no-tools "Continue without tools"
```

### Same-Turn Failure Handlers

Threads can configure a same-turn failure handler through the `set_turn_failure_handler`, `get_turn_failure_handler`, and `clear_turn_failure_handler` tools so terminal turn failures automatically continue the turn with custom recovery instructions up to the configured continuation limit. The `turn_failure_handlers` feature, on by default, controls the tools and continuations; temporary structured requests such as recaps turn it off.

### Reconnect Command Preservation

When disconnecting from a session, the exit summary preserves the session's active CLI flags and remote connection settings in the printed `codex resume` command so reconnecting restores the same configuration.

### Fork Maintenance

The repository includes helpers for maintaining and building the fork:

- `just rebase-upstream` rebases the current branch onto `upstream/main` after creating a backup branch; every run is followed by consolidating the fork stack into atomic commits
- `just rebase-status`, `just rebase-continue`, and `just rebase-abort` are convenience wrappers for an in-progress rebase
- `just audit-fork` starts the separate full fork-maintenance audit flow using the latest pre-rebase backup
- `just set-local-version --dry-run` previews the version derived from the latest included upstream release, upstream commit, and fork commit
- `just test-local` runs the test suite with verified V8 artifacts and an isolated `HOME` and `TMPDIR`
- `just build-local` rejects dirty source trees, applies that version temporarily, downloads and verifies the matching V8 artifacts, builds the current CLI and code-mode host, and restores the version files
- `just install-local` assembles the built binaries into a complete Codex package under `/usr/local/lib/codex`, updates the `/usr/local/bin/codex` symlink, repins an existing daemon to the new package, preserves one rollback copy, and prunes older local daemon packages

### Rebase and resolve conflicts

To update the fork, start from a clean `main` branch:

```shell
git switch main
git status --short
just rebase-upstream
```

If the rebase stops, resolve and stage the reported conflicts and run `git rebase --continue`, or return to the pre-rebase state with `git rebase --abort`. These native commands remain available even if the justfile itself is being replayed. The helper creates and prints a timestamped backup branch before rewriting anything.

### Consolidate the fork stack after every rebase

Every `just rebase-upstream` is followed by squashing the fork's commits on top of `upstream/main` into atomic units whenever possible. Each resulting commit holds one self-contained change, such as a feature or a fix, together with its tests, formatting, and follow-up fixes, and builds on its own. Fold formatting-only, fixup, and review-follow-up commits into the commit they belong to, and drop the previous `Stamp workspace with local version` commit if it is present. Keep unrelated changes in separate commits.

Review the stack with `git rebase -i upstream/main`. Where an interactive editor is unavailable, for example in an agent session, mark commits with `git commit --fixup=<commit>` or reorder them through a prepared todo list, then run `GIT_SEQUENCE_EDITOR=true git rebase -i --autosquash upstream/main`. Confirm the consolidated tree matches the rebased one with `git diff <rebased-head> HEAD`, which must be empty.

The range-diff review, the complete test suite, linting, and release builds are part of the separate audit flow below.

### Full fork-maintenance audit

Run this flow separately when you deliberately want a complete review of the fork:

```shell
just audit-fork
```

The helper selects the latest `backup/main-before-upstream-rebase-*` branch and prints the exact checklist and range-diff command. Pass a backup branch explicitly if you want to audit a different rebase:

```shell
just audit-fork backup/main-before-upstream-rebase-YYYYMMDD-HHMMSS
```

Start from a stack already consolidated as described in the previous section. Run the printed `git range-diff` command to compare the old fork stack with the updated one.

Run the focused tests needed for the fork changes, followed by the complete suite:

```shell
uv run python scripts/test_set_local_version.py
CODEX_REPO_ROOT="$PWD" uv run python scripts/test_build_local.py
uv run python scripts/test_install_local.py
CODEX_REPO_ROOT="$PWD" uv run python scripts/test_test_local.py
just test-local
```

`just test-local` runs the suite the way `just build-local` builds: it downloads and verifies the matching V8 artifacts, which a bare `just test` cannot fetch for the sandboxed V8 build. It also gives the tests an empty `HOME` and their own `TMPDIR` under `/var/tmp/codex-test-local` (or `CODEX_TEST_LOCAL_DIR` when set). That keeps the host user's skills and config out of snapshots, keeps temporary directories outside any git checkout, which tests of non-repository paths require, keeps dot directories out of temporary paths, which tests that encode paths into project names require, and keeps large fixtures from filling a small `/tmp`. Without the GStreamer 1.28 development libraries it leaves out `codex-voice-host`. Arguments replace the default `--workspace` selection, for example `just test-local -p codex-tui`.

Run scoped `just fix -p <crate>` checks for the affected Rust crates, then format the final stack:

```shell
just fmt
```

Build the locally versioned binaries, confirm the CLI's reported version, and install them as a local Codex package. `just build-local` requires a clean tree, temporarily stamps the workspace with the upstream and fork source commits, builds with the lockfile enforced, and restores `Cargo.toml` and `Cargo.lock` before returning. No version-stamp commit is created:

```shell
just build-local
codex-rs/target/release/codex --version
just install-local
```

Finally, update the fork remote:

```shell
git push origin main --force-with-lease
```

## Quickstart

### Installing and running Codex CLI

Run the following on Mac or Linux to install Codex CLI:

```shell
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

Run the following on Windows to install Codex CLI:

```shell
powershell -ExecutionPolicy ByPass -c "irm https://chatgpt.com/codex/install.ps1 | iex"
```

The standalone installers download from `https://releases.openai.com/codex` by default and fall back to GitHub Releases if a metadata or asset download is unavailable. To force GitHub Releases, set `CODEX_INSTALLER_USE_RELEASES_OPENAI_COM` to `false` (`0` and `no` are also accepted):

```shell
curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_INSTALLER_USE_RELEASES_OPENAI_COM=false sh
```

```powershell
$env:CODEX_INSTALLER_USE_RELEASES_OPENAI_COM='false'; irm https://chatgpt.com/codex/install.ps1 | iex
```

Codex CLI can also be installed via the following package managers:

```shell
# Install using npm
npm install -g @openai/codex
```

```shell
# Install using Homebrew
brew install --cask codex
```

Then simply run `codex` to get started.

<details>
<summary>You can also go to the <a href="https://github.com/openai/codex/releases/latest">latest GitHub Release</a> and download the appropriate binary for your platform.</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `codex-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `codex-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `codex-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `codex-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `codex-x86_64-unknown-linux-musl`), so you likely want to rename it to `codex` after extracting it.

</details>

### Using Codex with your ChatGPT plan

Run `codex` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Codex as part of your Plus, Pro, Business, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-codex-in-chatgpt).

You can also use Codex with an API key, but this requires [additional setup](https://developers.openai.com/codex/auth#sign-in-with-an-api-key).

## Docs

- [**Codex Documentation**](https://developers.openai.com/codex)
- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
