# M2 — language tools, validation and task state

Status: implemented and verified on the development Mac. The approved architecture and subscription requirement are unchanged. This milestone supplies language tools, local checks and operational task state; AI decomposition and review remain M3 work.

## Working plan

1. Extend the LSP foundation to a supervised project session with multiple versioned documents. Add Rust diagnostics, completion, hover, definition, references, symbols and explicit formatting. Reject stale results and unrequested workspace edits; recover from server failure without affecting editing.
2. Discover already-installed Rust tools and record a per-project trust decision before starting them. Start with build scripts/procedural macros and automatic Cargo checks disabled; use locked/offline Cargo metadata. Local language tools execute with the user's permissions, so trust is distinct from an AI edit grant or a sandboxed validation grant.
3. Materialize bounded validation snapshots, overlay unsaved buffers, and record input hashes/revisions. Exclude credentials, instructions/config that could alter Codex, ignored generated content, special files and symlinks. Use exact executable/argv recipe envelopes and the already-tested Codex permission-profile execution path. Permit artifact writes in an isolated run directory, deny original-project access and tool networking, and never import validation changes.
4. Persist project-tool settings, recipe authorizations, tasks/plans/steps and check receipts in SQLite. Back up before schema migration. Mark unfinished checks interrupted on restart; do not silently rerun them. Tasks record operational work, never learner scores.
5. Integrate Tools, Problems, Checks and Task controls into the native application. Keep ordinary editing independent of AI access. Use separate validation fixtures and app profiles, preserving the current M1 window.

## Concrete acceptance

- Real rust-analyzer results reflect unsaved code in more than one file; UTF-16 offsets and late responses are handled correctly.
- Formatting is an explicit undoable buffer edit and is rejected if its input version changed.
- A real Cargo test checks a captured unsaved version while originals remain unchanged. Receipts identify snapshot, recipe, executable/version, output, exit/cancellation and omitted inputs.
- Authorize a recipe once per unchanged envelope; broader or changed recipes require a new decision. Missing dependencies/tooling and interrupted runs remain unvalidated.
- Cancellation cleans up a running check; restart preserves task/check state. A failed language server or validation runtime leaves editing usable.

## Research baseline

Observed locally for this milestone: Codex CLI 0.169.0 and rust-analyzer 1.100.0. rust-analyzer exposes `--print-config-schema`; use that installed schema to verify configuration. Its documented `cargo.extraArgs` applies to Cargo invocations, with separate metadata arguments and options to disable build scripts, proc macros and check-on-save. [Official configuration reference](https://rust-analyzer.github.io/book/configuration.html).

Language tools are trusted local developer processes, not a claim of sandbox containment. Validation runs have a separate enforced filesystem/network profile. M2 does not introduce model turns, automatic dependency installation, broad refactoring, AI review or a plugin framework.

## Implemented behavior

The native app now has **Tools, Problems, Checks and Task** panels, reachable with the Tools & Task button or ⇧⌘L. The task is operational project state; no learner profile, score or cross-project learning history was added.

- Discover already-installed rustup toolchains by inspecting files. Honor the project's toolchain selection without invoking rustup or installing anything. Store project trust against executable fingerprints; reopen trusted projects with their language service enabled only when the fingerprint still matches.
- Supervise rust-analyzer in a separate process group. Synchronize all open Rust buffers, including unsaved text and reopened-document identities. Use bounded queues, request timeouts/cancellation and bounded automatic restart; offer manual restart/disable. Deny server-initiated workspace edits and external document opening.
- Bind ordinary symbol completion, hover and definition to the editor. Expose symbols/references as navigable results, explicit undoable formatting, and pull diagnostics after a quiet editing boundary (with a slower local refresh). No model review runs while typing. Results carry a context sequence and are discarded after edits or detected external changes; stale diagnostic decorations are cleared.
- Capture eligible project inputs and unsaved/new buffers into a private run directory outside the project. Respect ignored existing inputs, local ignore rules for new buffers, excluded generated directories, credential-like names, and instruction/config directories belonging to agents. Reject symlinks, hard-linked and special inputs with recorded omissions. Bound capture to 20,000 files, 8 MiB per input and 64 MiB overall.
- Authorize fixed Cargo check/test recipes once per project and unchanged execution fingerprint. Check executable identity again before execution. Use the pinned Codex 0.169.0 `command/exec` path, experimental named permission profiles, an ephemeral credential-free per-run home and no model turn. No API billing fallback is present.
- Give commands read access to the captured source, installed toolchain/Apple development files and dependency caches; write access only to isolated artifacts. Deny original-project access and tool networking. Use locked/offline Cargo, explicit compiler/linker/SDK paths and a 120-second command timeout. Retain bounded output, diagnostics, snapshot hashes, omitted inputs, exit/cancellation details and artifacts.
- Keep results as evidence about their captured version. A passing command does not complete a step. Failed tests, unavailable tooling/dependencies, cancellation and missing terminal results remain distinct. Close/project switch is held while a check is active; restart marks unfinished receipts interrupted without replaying the command.
- Persist plans/revisions, ordered step dependencies, explicit manual completion, tool trust, recipe grants and check receipts in SQLite schema 2. Back up a schema-1 database before migrating. Reject stale task/check writes. Saved tasks and checkpointed drafts survive restart.

The app's language trust and validation grants are separate from AI source-edit authority. The human terminal still has ordinary user permissions; it is not the isolated validation interface.

## Verification

See the [sanitized verification receipt](evidence/m2/verification.json) for commands, scope and evidence paths. There were **38 passing workspace tests**, with one intentionally ignored subprocess fixture exercised by its parent recovery test. Formatting, strict Clippy and the development build passed. The snapshot-specific tests were rerun after the final input-opening hardening.

**Deterministic and simulated checks** cover schema-1 backup and unsaved-draft preservation; durable progress/revisions and stale-write rejection; reconciliation of incomplete check records; task dependency cycles; snapshot exclusions and unsaved overlays; Unicode edits and overlap rejection; a delayed LSP response made stale by edits; unsolicited workspace edits; dropped-request cancellation; server crash/restart; and restoring the child signal mask without altering the parent thread. Existing M1 IO, terminal, protocol and crash-recovery tests remain passing.

**Real language integration** used rust-analyzer 1.100.0 against a two-file Rust fixture. Definitions, references, hover and symbols recognized an unsaved function name across files. Ordinary completion, diagnostics, formatting and restart/resynchronization passed; stale formatting stamps were rejected and original files remained unchanged. Initialization is not treated as completion of semantic indexing: the integration fixture waits for actual expected results within a bounded period.

**Real isolated checks** passed an unsaved implementation, reported an intentional failing assertion, cancelled a running test, and classified an unavailable offline dependency as unvalidated. A fixture build script verified denied reads/writes of the original project, denied writes through hard links to source/cache, denied connection to a live loopback listener, and successful writes to its artifact directory. All four receipts retained unchanged input hashes. These checks used no model authentication.

**Native UI observations**, in a separate `dev.tandem.m2qa` bundle/profile:

1. Enabled/restored tool trust; formatted a buffer and undid the formatting.
2. Submitted an unsaved alternative doubling implementation (`value + value`), ran Cargo tests, and saw a passing snapshot receipt while the disk version remained `value * 2`. This verifies test behavior, not an AI review's acceptance of alternatives; that gate remains M3.
3. Repeated the unchanged recipe without another authorization prompt. Cancelled a deliberately slow check and observed its exit/receipt.
4. Created a task with criteria and two steps; saved progress, restarted and recovered it. Explicit completion linked the step's check receipts and remained labeled as a user decision.
5. Confirmed the ordinary close guard while a check ran. Force-killed only the verified M2 test process, reopened the same profile, and observed the unsaved buffer and an **Interrupted** receipt with no automatic rerun. The old language/check child processes exited.
6. Typed incorrect unsaved Rust, observed a specific type diagnostic, followed its link, fixed the code and saw the old finding removed. Symbol navigation returned the expected functions.
7. Disabled language tools and edited/undid text with AI and language processes off. Inspected exposed accessibility labels for tool permissions, task criteria/status and receipt details.

The user's earlier M0 typing/VoiceOver reports remain separate evidence. M2 accessibility was inspected through the native accessibility tree and screenshots; spoken VoiceOver behavior was not re-verified by the user in this milestone. This is bounded agent-driven verification, not a broad usability or performance benchmark.

## Integration problems resolved

The first GUI check completed its child command but did not receive its terminal result: a macOS dispatch worker's blocked signal mask was inherited by app-server, preventing its asynchronous child reaper from observing the exit. The subprocess setup now clears the mask in the child only. A regression fixture verifies parent-mask preservation, and the real GUI recipe then completed in approximately two seconds for the small no-dependency fixture. The earlier stalled attempt is retained as historical test data; current unknown-outcome handling reports interruption rather than claiming cancellation completed.

Explicit Apple linker/SDK paths avoid an `xcrun` cache write outside the artifact boundary. Test-failure evidence takes precedence over incidental infrastructure-warning text. Hashing the pinned runtime is optimized in development builds rather than weakening its fingerprint check. Duplicate accessibility identities for repeated step/output labels were caught in the debug UI and corrected with stable container scopes; recovered state survived those test-process failures.

## Remaining limits and next milestone

- Rust only in M2. Python/Pyright and interpreter/test selection are M3 work. Proc macros and build-script expansion remain disabled in the language service; some legitimate projects therefore have incomplete semantic information. Compiler checks remain separate evidence.
- The local adapter currently expects the tested Homebrew Codex path, installed Rust tools and the Xcode.app Apple SDK. Named permission profiles are experimental. Broader discovery/distribution and compatibility gates belong before M5; no Windows support is claimed.
- Validation is offline and conservative. Dependencies must already be prepared explicitly. Symlinks/hard links, generated source written beside inputs, external path dependencies, missing lockfiles and unsupported ignore/layout cases can leave a project unvalidated. The app does not widen permissions to force a pass. New unsaved buffers apply local ignore policies conservatively; nested re-inclusion can require saving/rechecking the layout.
- Snapshots represent the files read during bounded capture, with each file's hash recorded; they are not a filesystem-wide atomic snapshot against unrelated concurrent writers. File events invalidate language context when observed. AI review applicability across full context/acceptance revisions remains M3 work.
- Plans can be created and revised manually. The richer per-step reasoning, file/symbol links, configurable assistance and AI review interface will be filled by M3. Plan revision history is stored, but there is no history browser yet. Unsaved plan-form edits require Save Plan; persisted plans and editor drafts have the documented recovery guarantees.
- Output is capped at 256 KiB and the panel renders its last 80 lines; Copy Check Output exposes the captured text. The latest 30 receipts load into the UI. Run artifacts remain in `~/Library/Caches/Tandem/checks`; retention/cleanup UX is still needed before daily use. This milestone does not claim rich benchmark/test discovery or full test-result parsing.
- The native M2 bundle is a local ad-hoc-signed development build. Testing and process termination targeted the separate M2 fixture/profile. The M1 bundle was not repackaged and its state remains on schema 1 with its drafts retained. The final M2 fixture window remains open for inspection.

Next is **M3: the companion loop with Rust and Python**, including explicit structured task state, settings inheritance, hidden/revealed solutions, snapshot-specific reviews, context selection and subscription connection/failure handling. The real subscription turn was proven in M0; this milestone did not consume a new model turn or change authentication.
