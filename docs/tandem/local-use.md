# Using the local Tandem release

Tandem is a standalone Rust app for macOS on Apple Silicon. A versioned package includes its verified Codex 0.169.0 runtime; Cargo, Homebrew, Node and Python are not required to launch or edit. Language servers, compilers and interpreters remain external tools configured for each project. See [guided companion evidence and limits](guided-companion.md) for the observed Rust/Python journeys and partial native checks.

## Install and launch

Package creation and installation are separate operations. After inspecting a concrete versioned bundle and choosing to install it:

```sh
python3 scripts/macos_release.py verify /absolute/path/to/Tandem.app
python3 scripts/macos_release.py install /absolute/path/to/Tandem.app
open "$HOME/Applications/Tandem.app"
```

The installer creates `~/Applications/Tandem Releases/<release-id>/Tandem.app` and atomically points `~/Applications/Tandem.app` at it. Existing versions and project/recovery data remain intact. It refuses an unrelated app or launcher link. Quit the old running version before launching another; the installer never overwrites a running version's files. An installation receipt is stored in `Tandem Releases/installation.json`. Building or verifying a package does not switch this launcher or migrate the user's profile.

This is a local ad hoc signed build. It is not notarized or tested for distribution to another Mac. No signing credentials, global environment changes, browser-session scraping or API-key billing are involved.

## Discuss, approve and implement

Open a project with ⌘O. In **Companion**, connect the existing ChatGPT subscription or use its managed sign-in flow. A new task is a saved conversation from its first message. Discuss the goal, ask follow-ups and answer real clarification questions using one of two to five suggestions or custom text. General discussion remains available while a question is pending. Ordinary chat never approves documents or starts implementation.

Research can inspect relevant project documentation, dependency metadata and fixed read-only Git/tool recipes. Public research uses a screened brief containing generic topics, queries and URLs in a separate public worker. That worker receives no private transcript, repository code, local paths or task documents. Anonymous source retrieval enforces public-network, content and size limits. Activity, citations and unavailable/partial receipts show what actually happened; a search result alone is not a retrieved source. Research grants no edits, check execution, approvals or Takeover authority.

Use **Draft Spec** and **Draft Plan** to request proposals, then inspect their separate editable views. Save and explicitly **Approve Spec** and **Approve Plan** for their exact displayed versions. The plan must reference the current spec. A changed spec reopens both approvals and makes the former plan outdated; a changed plan reopens its own approval. No-op saves preserve approvals. Unsaved edits cannot be approved. Both current approvals enable a separate **Start Implementation** action.

Start prepares one concrete action in the first dependency-ready step. Each action explains its file/symbol, exact command/cwd or validation criteria, expected result and required evidence. Open/Create in editor and Copy Command are explicit human actions. Complete the edit or command, then request **Review**. A current passing AI review enables **Next**; clicking it advances exactly one action. Missing, failed, stale or incomplete evidence keeps Next disabled. Passing Review never advances automatically, and inspecting another outline step cannot skip the current action. Final completion requires the full-spec assessment. Completed actions, prior reviews and failed attempts remain history; they do not authorize current work.

Solutions start hidden. Hint/Explain and normal conversation do not Reveal. A separate Reveal targets only the current action/language and changes no files. An explicitly selected AutomaticPreview preference is standing permission for that preview only after current approvals and Start; it grants no editing authority. The default remains hidden.

The model preference is global: **Automatic** resolves the live catalog's unique account default and supported default reasoning effort at run start, or select an available explicit model. An unavailable explicit choice remains selected and blocks AI until refreshed or changed; Tandem never silently substitutes another model. The picker is disabled during any active AI operation, including research and Takeover. Local phase/loading indicators identify the initiating operation. **Cancel** or ⌘. requests stopping; the UI retains completed effects and shows Stopping until reconciliation. Failures and restart do not automatically replay an uncertain turn.

Legacy task plans/reviews remain readable as history. Continuing their implementation creates a linked conversation and requires the missing Spec and dependent Plan to be drafted, saved and explicitly approved, then Start. Historical manual completion is never converted into a passing action review.

## Editor, checks and Takeover

Use tabs, ⌘P quick open, ⌘F find, ⇧⌘F project search and ⌘S save. Draft recovery is local and automatic; it does not automatically save project files. The **Changes** view compares buffers with disk. Disk conflicts preserve your draft for comparison or Save Copy. Open/edit/save, undo, search, diagnostics and the human terminal remain independent of AI approvals and sign-in.

Use **Tools** to select installed Rust/Python tools and authorize a trusted project. These language servers run with your user permissions. **Checks** runs separately authorized recipes in isolated snapshots, including eligible unsaved buffers. Check results identify their input/provenance; a model's claim is not a check receipt. Manual **Attach Output** records exact command, cwd, exit and output as user-supplied evidence, not authenticated execution. The human terminal is a normal local shell acting on the live project with your permissions. Local checks need no model sign-in.

**Takeover** (⌥⌘T) requires current document approvals and Start plus its own explicit scope authorization. Prepare exact writable paths and checks, inspect the scope, then authorize. Edits remain in an isolated copy until you preview/import them into unsaved buffers. Import blocks overlapping changes. Imported buffers still require the current action's AI review; Takeover cannot complete walkthrough actions. Changed documents invalidate the old workflow grant while preserving staged work. ⌥⌘Z previews the inverse of the latest import; normal buffer undo remains separate.

**Clarity / Speed / Balanced** make proposals under the current workflow gates. Actual benchmark receipts are shown separately from expected complexity. Viewing or revealing a proposal never inserts code or authorizes Takeover.

## Health, recovery and rollback

Open **App** or press ⌘, for verified runtime identity, connection/tool status, recovery locations and notices. A missing/damaged bundled runtime disables AI and isolated validation; editing remains available. Repair by installing a verified release rather than replacing an individual bundled executable.

Recovery normally uses `~/Library/Application Support/Tandem/workspace`; disposable test profiles use `--state-dir`. Credentials are managed by Codex in Tandem's separate credential home and Keychain entry and are never displayed in the UI. Staged work and validation artifacts live under `~/Library/Caches/Tandem`. Keep staging until its import is resolved. Cache cleanup is manual in this release.

This release uses database **schema 5**. Opening an older profile creates a pre-migration backup before upgrading. Older apps refuse newer schemas without rewriting their data. Rollback only changes the launcher; it does not downgrade the database. To return to a previous managed version after choosing a compatible profile:

```sh
python3 scripts/macos_release.py rollback
```

Quit before switching builds. A schema-4 app cannot reuse a schema-5 profile: reinstall the newer app or give the older app a separate `--state-dir`. Do not overwrite the current database to force a downgrade. One process owns a profile at a time; live-owner exclusion and release after child-process descriptor inheritance are regression tested.

## Current limits

UTF-8 files up to 2 MiB, 32 open tabs, bounded repository/context capture and one foreground AI operation. No persisted per-buffer undo history, cursor/scroll restoration or general unsent form drafts; the conversation composer has its own saved draft. Import undo journals survive restart. Terminal rendering is monochrome. Python formatting is not supplied by Pyright. The fixed comparison benchmark supports Python callbacks; arbitrary benchmark commands and Rust benchmark integration are later work. No debugger, extension marketplace, updater, Windows support or cloud/team accounts are included.

Included ChatGPT usage limits apply. A connection or usage failure preserves work, and Tandem never silently switches to API billing or another provider. Completed real fixture journeys establish the recorded cases, not broad model quality. Native verification remains partial: spoken VoiceOver, all live cancellation phases, busy picker behavior, native output attachments/review/Next, long-panel scrolling and full native journeys remain unverified. See the [scenario checklist](guided-companion.md#disposable-native-seed-and-manual-checklist).

## Build and verify another local release

On the development Mac with Rust 1.100.0 and the tested vendor Codex package already available, first commit the intended source and record that clean HEAD. Choose `guided-companion-20261114-<short-head>` using its actual short commit ID. Never overwrite an existing release. For example, after replacing `SHORT_HEAD` with that recorded ID:

```sh
python3 -m unittest discover -s scripts/tests
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo build --release -p tandem
python3 scripts/prepare-notices.py
python3 scripts/collect-notices.py --output .tandem-local/guided-companion/ThirdPartyNotices
python3 scripts/macos_release.py package --release-id guided-companion-20261114-SHORT_HEAD --notices .tandem-local/guided-companion/ThirdPartyNotices
python3 scripts/macos_release.py verify .tandem-local/releases/guided-companion-20261114-SHORT_HEAD/Tandem.app
.tandem-local/releases/guided-companion-20261114-SHORT_HEAD/Tandem.app/Contents/MacOS/tandem --runtime-status
```

Preparation downloads public source/notice inputs pinned in `scripts/release-inputs.json` and `scripts/notice-sources.json`; Cargo may populate dependency caches. It does not compile upstream Codex or install/change a global toolchain. Its historical `.tandem-local/m5` source/cache paths are still used by the existing collector. Collection covers the current lockfile, including public HTTP/TLS/HTML research dependencies and normal/build edges with the conservative workspace feature superset. Preserve declared-license limitations, MPL source archives, notice hashes and the zero-unresolved inventory. See [third-party notices](third-party-notices.md).

Keep bundle, signed inventory, exact source HEAD, binary/runtime hashes and QA receipts private under `.tandem-local/`. `--runtime-status` checks discovery without opening the GUI or reading credentials. The required Codex fingerprint is `739cda86e15d30a6e2b40cf8dcbbfb988aaf48b305b27065573bb8e9fd41d002` for version 0.169.0. Verification checks the ad hoc signature and inventory; it does not prove native usability or notarization.

For offline native QA, run `~/.cargo/bin/cargo run -p tandem --example guided_ui_seed` and launch the concrete new bundle's `Contents/MacOS/tandem` with the exact emitted project and `--state-dir`. Preserve the user's launcher/profile, identify the QA PID/window, and record only observed behavior. The seed uses fake persisted responses and consumes no subscription turns. Real subscription examples require their separate bounded authorization; release packaging does not authorize rerunning them. Installation remains the separate optional command shown above.
