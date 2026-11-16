# M1 — dependable local editor

> **Historical.** This document describes the earlier Codex-based design, which was replaced on 2026-11-15 by the Claude chat companion. The current design is in [companion.md](companion.md).

Status: implemented, with focused automated and native UI verification on the development Mac. The approved architecture is unchanged. This is an early local editor, not the first companion-equipped release.

## Working plan

1. Add a small, testable workspace/buffer/storage core: bounded UTF-8 loading, BOM/newline preservation, version-checked atomic saves, immutable recovery snapshots, and SQLite workspace state outside the project.
2. Build the real `tandem` desktop binary using the proven native editor. Add folder opening/recent projects, ignore-aware file navigation and quick-open, tabs, new files, find/replace, explicit project search, and disk-versus-buffer changes.
3. Share the proven terminal component between the probe and application. Opening a folder will not automatically start a shell, language server, or model. A user-started terminal runs in the selected project.
4. Reconcile external filesystem changes using notifications plus periodic hash checks. Preserve dirty buffers on conflicts. Checkpoint drafts off the UI thread and flush before close/quit; restore them without writing to project files on startup.
5. Verify meaningful IO/recovery/conflict cases, compile/lint, exercise a packaged application against disposable projects, and perform the planned sustained coding check. Keep AI task workflows and broader LSP management for M2/M3.

## Acceptance focus

- Multiple files retain independent undo histories and dirty state.
- Saving does not overwrite a changed/deleted/replaced disk baseline silently; failed saves retain the draft.
- Restart/forced termination recovers checkpointed unsaved content, active tabs and project choice; recovery never overwrites external edits.
- Read-only, binary, oversized and escaping paths produce clear errors.
- Local editing/search/terminal work with no AI authentication or process.

Recovery uses SQLite now so M2 can extend the same local store. The editor owns live text; the core receives immutable snapshots for saves, persistence and reconciliation. Filesystem-wide compare-and-swap against unrelated external writers is not promised; saved baselines and retained drafts protect detected conflicts.

## Implemented behavior

- Open a real folder through the native dialog, a recent-project entry or a command-line path. Navigate an expandable file list, filter with quick-open, and create files in existing directories.
- Edit multiple UTF-8 files with independent editor state, syntax highlighting, undo/redo, native find/replace and project search that includes captured unsaved buffers. New file contents remain in memory until saved.
- Save with baseline hash checks and same-directory atomic replacement. Preserve BOM, existing CRLF style, mixed existing line endings and executable permissions. Stop on conflicting disk contents, deletion, read-only permissions and unsupported input; retain the draft.
- Compare disk and buffer contents. Explicitly choose the disk version with session undo available, or save a draft copy through the native save dialog.
- Store workspace metadata and dirty snapshots in SQLite outside the project. Use one transaction for each checkpoint and monotonic checkpoint sequences so delayed background work cannot overwrite a newer flush. Flush before project switching and closing.
- Restore open tabs, active file, font size and checkpointed drafts after restart. Recovery never writes those drafts into the project automatically. A draft remains separately inspectable when disk changed while the app was closed.
- Reconcile open files using filesystem notifications and periodic hash checks. Rescan the index on events, explicit refresh and a slower fallback. Refresh clean buffers without discarding session undo; preserve dirty ones on conflict.
- Keep live buffers and undo when the current project is reselected. During an actual project change, move focus away from the old editor, prevent new file actions, and checkpoint again before replacing workspace state.
- Run an explicitly started human terminal in the project folder. Reuse the M0 terminal engine/PTY, with bounded scrollback, visible-output copy, interrupt, stop/close and resizing. A running foreground command blocks project switching until interrupted or closed.

The editor owns live text. The core receives immutable snapshots for IO and persistence. The application does not start Codex, an LSP server, a shell or project build scripts merely because a folder was opened.

## Validation

**Automated:** 26 tests passed across the workspace: 14 workspace/storage cases, 3 terminal cases, 7 Codex transport cases and 2 LSP framing/position cases. One additional test entry is intentionally ignored by the normal runner because it is a subprocess fixture; the forced-termination test launches it explicitly and kills it after its checkpoint is committed.

The M1 cases cover byte/permission preservation, conflicting external edits and deletion, concurrent creation, mixed newlines, ignored files, bounded/cancelled searches, unsaved search overlays, restart recovery, stale checkpoint rejection, forced process termination, changed permissions and refusing a newer SQLite schema without rewriting its data. Terminal tests cover bounded history, wide/combining Unicode and foreground-process cleanup.

**Native application checks:** the packaged `.app` was exercised with separate disposable projects and an isolated state directory. Observed outcomes included:

1. Open and edit Python/Rust files; save, undo after save, switch tabs and retain separate contents.
2. Search for an unsaved marker and navigate a result.
3. Modify disk externally; see save stopped and both versions displayed in the comparison.
4. Force-kill the verified test process after checkpointing two dirty files; reopen without a folder argument and recover both drafts, preserving the external disk version.
5. Choose the disk version explicitly, then undo/redo that choice within the session.
6. Create/save a new Rust file, use native find/replace, undo a replacement, and invoke project search through its corrected shortcut.
7. Edit a read-only file, observe the blocked save, and save a copy through the native dialog. A separate filesystem assertion confirmed the original was unchanged and the copy contained the draft.
8. Open another project through the native folder picker, then restore the previous project and its drafts through Recent.
9. Start a terminal explicitly; verify its working directory and Cargo discovery, run output-producing commands, navigate scrollback, return to live output, interrupt a foreground command, and stop the terminal.

Interactive verification spanned more than an hour and was interleaved with implementation/debugging and rebuilds. It was agent-driven testing of concrete workflows, not a continuous typing benchmark or a claim of long-term daily usability. The user's earlier M0 typing and VoiceOver feedback is recorded separately; it is not counted as new M1 feedback.

Build, formatting, strict Clippy and evidence checks are recorded in [the M1 verification receipt](evidence/m1/verification.json). The development bundle was launched through macOS Launch Services. No model turns or API-key billing were needed for M1 editing.

## Delivery and use

Build/run instructions are in the [repository README](../../README.md). M1 packaging produced `.tandem-local/m1/Tandem.app` with an ad-hoc signature. The current script defaults to M2; targeting the historical M1 destination now requires `--profile m1`. Packaging replaces the executable atomically, so rebuilding does not truncate the executable mapped by a running test instance.

Normal state: `~/Library/Application Support/Tandem/workspace`. Validation state: `.tandem-local/m1/state`. The current test window and its drafts were left intact after possible concurrent user activity was noticed. These profiles are separate; opening a normal app instance does not delete the validation profile.

## Practical limits

- Recovery is checked approximately every 500 ms and written asynchronously. “Drafts backed up” reflects an acknowledged checkpoint. A forced kill before acknowledgment can lose the newest uncheckpointed input. A failed checkpoint keeps the window open on an ordinary close and reports the error.
- Saves are atomic per file; Save All is not a multi-file filesystem transaction. Hash rechecks detect ordinary conflicts but cannot provide filesystem-wide compare-and-swap against an unrelated writer in the final rename race window.
- UTF-8 only, up to 2 MiB per disk file; 32 open tabs; up to 20,000 indexed files, bounded traversal depth/entry count, 250 displayed navigation rows and 200 search results. Filtering and exact relative paths make additional indexed/ignored files reachable. Binary and unsupported files remain unchanged.
- Tabs, active file and font size restore. Cursor/scroll position, arbitrary pane layout and cross-session undo do not yet restore. “Keep Draft & Close” retains the draft for the next workspace open.
- The terminal is a basic `/bin/sh` view with monochrome cells and visible-output copying. Full text selection, comprehensive terminal IME and a complete terminal-emulator UI remain later work. It is a human shell with OS permissions, not an AI execution tool.
- Broad accessibility, international input, very large repositories and sustained daily coding still need further evaluation. No Windows support is claimed.
- LSP management, check recipes, task plans, configurable guidance, AI review and takeover are M2–M4 work. No cloud backend, plugin platform or learning-progress tracking was added.

## Next milestone

M2 adds real project language-tool management, snapshot-based validation recipes and durable task state. It will reuse the M0 integration/permission evidence and the M1 workspace store, with a short implementation plan before changes. No architecture or subscription change is currently recommended.
