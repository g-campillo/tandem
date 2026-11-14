# Using the local Tandem release

Tandem is a standalone Rust app for macOS on Apple Silicon. The M5 package includes its verified Codex 0.169.0 runtime; Cargo, Homebrew, Node and Python are not required to launch or edit. Language servers, compilers and interpreters remain external tools configured for each project.

## Install and launch

Use the versioned bundle produced by `scripts/macos_release.py`. Install it with:

```sh
python3 scripts/macos_release.py install /absolute/path/to/Tandem.app
open "$HOME/Applications/Tandem.app"
```

The installer creates `~/Applications/Tandem Releases/<release-id>/Tandem.app` and atomically points `~/Applications/Tandem.app` at it. Existing versions and project/recovery data remain intact. It refuses an unrelated app or launcher link. Quit the old running version before launching another; the installer never overwrites a running version's files. An installation receipt is stored in `Tandem Releases/installation.json`.

This is a local ad hoc signed build. It is not notarized or tested for distribution to another Mac. No signing credentials, global environment changes, browser-session scraping or API-key billing are involved.

## Code with the companion

Open a project with ⌘O. Use tabs, ⌘P quick open, ⌘F find, ⇧⌘F project search and ⌘S save. Draft recovery is local and automatic; it does not automatically save project files. The **Changes** view compares buffers with disk. Disk conflicts preserve your draft for comparison or Save Copy.

Use **Tools** to select installed Rust/Python tools and authorize a trusted project. These language servers run with your user permissions. **Checks** runs explicitly authorized recipes in isolated snapshots, with artifact writes declared separately. The human terminal is a normal local shell; its commands act on the live project with your permissions.

In **Companion**, connect the existing ChatGPT subscription or use its managed sign-in flow. Analyze and accept a task plan, work on a step, ask questions, reveal an optional read-only suggestion, then review and validate. Hidden solutions stay hidden until requested. Guidance settings do not grant editing authority. Compiler/test results, model assessments and your completion decision are separate.

Use **Takeover** (⌥⌘T) only when you want to delegate: prepare exact source paths and checks, inspect the scope, then authorize. Edits remain in an isolated copy until you preview and import them into unsaved buffers. Import blocks overlapping changes. ⌥⌘Z previews the inverse of the latest import; normal buffer undo remains separate. ⌘. stops AI work and retains completed actions. Nothing automatically resumes after a failure or restart.

**Clarity / Speed / Balanced** make proposals. Actual benchmark receipts are shown separately from expected complexity. Revealing or viewing a proposal never inserts code or authorizes takeover.

## Health, recovery and rollback

Open **App** or press ⌘, for verified runtime identity, connection and tool status, recovery locations and third-party notices. A missing/damaged bundled runtime disables AI and isolated validation; editing remains available. Repair by installing a verified release rather than replacing an individual bundled executable.

Recovery normally uses `~/Library/Application Support/Tandem/workspace`; test profiles use `--state-dir`. Credentials are managed by Codex in Tandem's separate credential home and Keychain entry and are never displayed in the UI. Staged work and validation artifacts live under `~/Library/Caches/Tandem`. Keep staging until its import is resolved. Cache cleanup is manual in this release.

To return the launcher to the previous managed version:

```sh
python3 scripts/macos_release.py rollback
```

Quit before switching builds. Rollback preserves data and never rewinds the database. An older app refuses a newer database schema; in that case reinstall the newer app or launch the older version with a separate profile. Schema migrations retain a pre-migration backup. Do not overwrite the current database to force a downgrade.

## Current limits

UTF-8 files up to 2 MiB, 32 open tabs, bounded repository/context capture and one foreground delegated task. No persisted per-buffer undo history, cursor/scroll restoration or unsent form drafts. Import undo journals do survive restart. Terminal rendering is monochrome. Python formatting is not supplied by Pyright. The fixed comparison benchmark supports Python callbacks; arbitrary benchmark commands and Rust benchmark integration are later work. No debugger, extension marketplace, updater, Windows support or cloud/team accounts are included.

Included ChatGPT usage limits apply. A connection or usage failure preserves work, and Tandem never silently switches to API billing. Ordinary editing works with AI disconnected.

## Build another local release

On the development Mac with Rust 1.100.0 and the tested vendor Codex package installed:

```sh
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo build --release -p tandem
python3 scripts/prepare-notices.py
python3 scripts/collect-notices.py --output .tandem-local/m5/ThirdPartyNotices
python3 scripts/macos_release.py package --release-id YOUR-NEW-RELEASE-ID --notices .tandem-local/m5/ThirdPartyNotices
python3 scripts/macos_release.py install .tandem-local/releases/YOUR-NEW-RELEASE-ID/Tandem.app
```

Preparation downloads public source/notice inputs pinned in `scripts/release-inputs.json` and `scripts/notice-sources.json`; Cargo may populate its dependency caches. It does not compile upstream Codex or install/change a global toolchain. Notice collection includes build dependencies and the workspace feature superset, so it deliberately over-includes some components. Ten inspected packages declare permissive terms without a standalone notice; their attribution files say so and include the declared standard terms and available original copyright statements. The package is for local use, without a claim of independent legal review.

Use a fresh release ID. To verify an existing concrete bundle, run `python3 scripts/macos_release.py verify /path/to/Tandem.app`. `Tandem.app/Contents/MacOS/tandem --runtime-status` checks runtime discovery without opening the GUI or reading credentials. Start the managed launcher by its full path; older retained releases share the application identity, so an OS lookup by name alone may select an older version.
