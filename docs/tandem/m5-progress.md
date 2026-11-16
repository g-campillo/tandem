# M5 — local macOS release

> **Historical.** This document describes the earlier Codex-based design, which was replaced on 2026-11-15 by the Claude chat companion. The current design is in [companion.md](companion.md).

Status: implemented and verified for local macOS use; continuation authorized after M4. No architecture or subscription changes.

## Implementation plan

1. Centralize pinned runtime discovery and verification in `tandem-codex`. Installed apps resolve Codex beside their resources; development commands retain the pinned local installation fallback. A missing/corrupt bundled runtime disables AI/check execution with a useful error instead of silently choosing another executable. Keep the managed credential home, included-usage gate and restrictive profiles unchanged.
2. Build an immutable, versioned Apple Silicon app package with the required Codex executables, manifests and license notices. Preserve upstream binary signatures/hashes. Verify actual model and sandboxed command turns from the relocated bundle with a minimal PATH; do not assume the CLI package is a single binary. Do not ship unused voice/code-mode resources without a concrete need.
3. Generate a locked dependency/source/license inventory for the shipped Rust graph, editor assets/grammars and selected Codex distribution. Resolve missing notices before calling the package complete. Include notices/resources in the signed app and record hashes. This is a local ad hoc build, not a claim of notarized distribution or Windows support.
4. Add a native application-health view: app/runtime identity, connection state, installed language/check tools, profile paths, recovery/cache locations and known limits. Make recovery, check output and takeover handoff easy to reach. Keep ordinary editing available without AI or a working runtime. Persist useful workspace/form state if it can be done safely without expanding educational tracking.
5. Provide a local installer and rollback path under the user's Applications directory. Never overwrite an unrelated app, a running executable, or an existing version. Retain earlier releases and operational data; rollback never silently downgrades a newer database. Include a versioned receipt and clear local-use instructions. No global environment, Codex configuration, signing account or updater service changes.
6. Run focused tests for runtime resolution/tampering, release manifests/install rollback and meaningful recovery changes, then the workspace checks. Exercise Finder launch with a clean environment, disconnected editing, Rust/Python tools, real subscription guidance and scoped takeover, check cancellation, restart recovery, process cleanup and a sustained 60-minute local coding session. Record what was observed separately from automated tests and user feedback; fix consequential usability failures before marking M5 complete.

## Acceptance boundaries

- The installed app uses its bundled, verified 0.169.0 runtime; local model turns still use the existing ChatGPT subscription with no API-billing fallback.
- Validation and takeover enforce the M3/M4 boundaries from the packaged location. No source, draft or unrelated global settings are lost during install, failure or rollback.
- Toolchains/language servers remain explicitly external dependencies. Missing tools produce actionable status rather than installation or execution outside authorization.
- The app and license resources pass manifest/signature checks, a previous release remains available, and the local launch path does not require Cargo or Homebrew on PATH.
- Daily-use evidence includes actual native interaction and runtime behavior. A build, model list or idle timer alone does not establish usability.

Largest uncertainties: Codex resource lookup/signatures after relocation; completeness of third-party notices; native focus/recovery behavior over longer work; safe rollback with durable local state. M6 self-development remains a separate milestone.

## Implemented result

The local release is an optimized Rust app with Codex 0.169.0 and its exec-server bundled under `Contents/Resources/codex`. Both vendor executables retain their original fingerprints. Runtime discovery is relative to the executing app, including case-insensitive `.app` suffixes; a damaged bundle never silently selects Homebrew or PATH. Development binaries keep the explicit pinned development path. The managed ChatGPT credential home, usage gate, model tool restrictions and sandbox profiles are unchanged.

The **App** panel (⌘,) shows app/release/runtime identity, connection and tool status, recovery/artifact locations and third-party notices. Failed connections retain visible retry controls. The companion's stop control also signals an active takeover. Ordinary editing and language tools remain available without a usable Codex runtime.

`scripts/macos_release.py` packages, verifies, installs and rolls back immutable releases. The managed launcher is `~/Applications/Tandem.app`; concrete bundles and the installation receipt are under `~/Applications/Tandem Releases/`. Updates replace only the launcher link. Existing app versions, running executables and recovery databases are preserved. Concurrent installer calls, unrelated destinations, corrupted resources, unexpected files and escaping paths are refused. Rollback does not rewind or delete operational data.

New File now supports a nested unsaved path without creating folders. Explicit Save creates validated parents. New-path checks reject escaping links, non-directory parents and later-arriving files. Benchmark callbacks can import their captured project modules through an explicit read-only snapshot path; inherited Python path settings remain ignored. Case iteration stops at the supported limit before workload setup.

## Evidence and fixes

| Verification | Actual result |
| --- | --- |
| Workspace checks | **69 Rust tests passed**, no failures; formatting and strict all-target Clippy passed; optimized build succeeded |
| Release/harness tests | **7 Python tests passed**: version preservation/rollback/idempotence, concurrent install exclusion, corruption/path rejection, captured-project imports, unequal-output rejection and bounded benchmark cases |
| Runtime relocation | Health view and minimal-PATH command verified the bundled location. Real sandboxed Cargo/Python commands and real subscription model turns completed from packaged paths |
| Native coding | Edited, undid/redid, saved and formatted Rust; added a function and relevant boundary tests; navigated Rust/Python symbols; searched across files; added a Python helper/tests; ran both check recipes and the human terminal |
| Companion/takeover | Real review accepted the manual `value + value` alternative using matching tests. Installed takeover created `checked_double` and its boundary tests, preserved existing functions and passed its authorized Cargo test |
| Import/recovery | Changed a buffer after preview and received a fresh-confirmation requirement. Imported two files into drafts; forced termination recovered three drafts and the journal. Validation passed on the recovered combined snapshot. Undo retained both independent user notes and removed only agent changes |
| Cancellation | A deliberately slow unsaved Python test reached Running and was cancelled with exit 137; source stayed unchanged and buffer undo restored the test draft |
| Missing runtime | A disposable bundle with its exec-server removed refused runtime discovery and connection without fallback. Native editing/save and Rust symbol navigation still worked; no model turn started |
| Installation/rollback | Finder launched the installed app. Actual launcher rollback switched between retained bundles without touching recovery data. A future-schema fixture was refused without modifying its database |
| Dependencies | Optimized executable inspection found only system dynamic libraries. The app launches without Cargo, Homebrew, Node or Python on PATH; language/check tools remain explicit external dependencies |

Resolved failures are retained in private logs/receipts:

- A transient app-server `-32603` connection error recovered on explicit retry. A minimal-PATH relocated probe and subsequent real native turns succeeded. The first error's underlying cause remains unconfirmed; no automatic model-turn replay or billing fallback was introduced.
- The strict M4 journal version guard exposed a missing increment when recording an agent check intent. Staged edits were retained and live source stayed unchanged. `record_check_intent` now advances the version before persistence; a regression test and a new real installed-app takeover passed.
- Package verification initially included the generated code-signature resource file in the resource manifest. Generated signature data and the signed main executable are now excluded from that self-referential inventory; macOS signature verification covers the executable/seal, and the manifest records the input binary hash plus other file hashes. Both vendor executable hashes remain independently pinned.
- A macOS `/var` → `/private/var` alias exposed inconsistent installer receipt paths; installation roots are now canonicalized, and idempotent installs retain the previous rollback target.
- New File required a parent directory even though imported drafts supported nested paths. Native reproduction, focused path-safety tests and native creation/save verified the fix.
- The first project-backed benchmark failed with `ModuleNotFoundError: No module named 'logic'` and correctly remained **Unvalidated**. The fixed harness adds only the captured project root; deterministic imports/equivalence tests and a new native measured run passed. The changed harness required a fresh recipe grant.

Two ordinary Rust-test entries are intentionally ignored: the crash helper is called by its parent test, and the real transport-loss test was already exercised in M3 and was not repeated because the transport implementation did not change. The real M5 runtime/native checks above were performed separately from unit tests. No new spoken VoiceOver assessment, second-Mac distribution test or Windows support is claimed.

## Notices and distribution boundary

The package includes a conservative **1,490-entry** source dependency inventory for Tandem and the two selected Codex binaries, including normal/build edges and workspace feature unification. It is not represented as a binary-level SBOM. Available original notices, separate Lucide icon terms, and complete unmodified source archives for MPL-declared components are included. Ten packages declare permissive terms but omit a standalone notice at the inspected source revision; their attribution files explicitly record that limitation, metadata authors and available original copyright statements alongside the declared standard terms. There is no claim of independent legal review.

Source and additional notice inputs are pinned by `scripts/release-inputs.json` and `scripts/notice-sources.json`. `prepare-notices.py` reconciles only upstream Codex's internal workspace version entries and verifies that all external lock pins/checksums match the release lockfile. Packaging rejects a notice inventory that differs from Tandem's Cargo.lock. See [notice methodology](third-party-notices.md).

This is a local ad hoc signed Apple Silicon build for the exercised macOS 27 environment. Notarization, other Macs, Windows, automatic updates and signing-account setup remain outside this release. Toolchains and language servers are not bundled or silently installed. Unsent forms, per-buffer undo stacks and cursor/scroll positions do not persist; task/draft/import state does. Cache cleanup remains manual and artifacts are inspectable from App. Older retained bundles share the app identity; launch the managed full path to select the current version.

## Handoff

Use [the local guide](local-use.md) for installation, normal coding, recovery and rollback. Sanitized counts, package hashes, actual check/model outcomes and the coding-session timeline are recorded in [M5 verification](evidence/m5/verification.json). Detailed QA logs and fixture state remain under `.tandem-local/m5/`; no credentials, authentication URLs or raw protocol streams are copied into documentation.

M6 is next: develop Tandem inside Tandem across two bounded real changes while keeping this release available for recovery.

## Session duration and delivered version

Delivered release: **0.1.0-m5.1**, selected by `~/Applications/Tandem.app`. The structured coding/validation session ran from 21:54:47 to 23:37:16 UTC on 2026-11-14 (102.5 minutes). Three native automation calls stalled for 29.9 minutes in total; those intervals are excluded, leaving 72.6 minutes. The timeline records concrete native edits, navigation, checks, failures, fixes, imports and recovery. It also included release engineering outside Tandem; this is not a claim of continuous human typing or M6 self-development. No idle sleep was used to satisfy the duration gate.
