# Implementation roadmap

> **Historical.** This document describes the earlier Codex-based design, which was replaced on 2026-11-15 by the Claude chat companion. The current design is in [companion.md](companion.md).

Status: approved on 2026-11-13; execution starts with M0. See [current evidence and progress](m0-progress.md). The acceptance criteria below are requirements, not claims of completed testing. [Plan entry point](README.md).

Codex implements the approved milestones end to end, including focused verification and local usability fixes. Before each meaningful milestone/change, write a brief plan naming scope, tests and expected result. Routine choices inside approved scope do not need another approval. A change to architecture, subscription access, source protection or major product scope does.

Use gates, not a speculative delivery date. M0 establishes the dependency/toolchain baseline and exposes integration cost; only then estimate the remaining work from actual build and editor results. Do not postpone a failed gate behind UI polish. Keep a short milestone receipt with versions, commands, results, limitations and links to artifacts; “documented” never becomes “tested” without evidence.

## M0 — Prove the foundations

**User-visible result:** a feasibility receipt showing whether Tandem can use the subscription, enforce boundaries and support reliable native editing. These are small disposable probes after approval, not the initial product shell.

**Dependencies:** initial plan approval. Use the installed Codex 0.169.0 as the first protocol baseline. Select a compatible Rust toolchain locally to the project if needed; do not modify global Rust defaults or global Codex configuration. Application scaffolding/dependencies become permitted only after approval.

### M0-A: subscription and runtime contract — first implementation work

**Largest uncertainty:** whether a real turn completes through Tandem's intended Rust client and managed ChatGPT authentication, with durable continuation.

Tasks:

1. Record binary version/source and derive stable/experimental schemas from that executable. Implement the smallest Rust stdio handshake/transport probe and bounded event capture with redaction.
2. Create a Tandem-owned Codex home/config; inspect credential-store key derivation and test isolation before any sign-out behavior. Use managed browser sign-in if required. Omit API-key/provider override variables.
3. Submit a harmless prompt whose content requires a real answer, await final successful turn status and retain a sanitized answer/event receipt. Continue with a follow-up that depends on that answer.
4. Shut down the child, restart, resume the thread and complete another turn. Exercise interruption on a bounded request; test normal connection loss separately through simulation.
5. Record account auth mode without credentials, chosen available model, thread/turn IDs, final statuses and binary/schema identifiers. Check isolated configuration and global configuration have not been changed unexpectedly.

**Acceptance:** real nonempty completed response and context-dependent follow-up under verified ChatGPT auth; restart/resume works; no API key or billing fallback; cancellation reaches a terminal or explicitly unknown state; no credentials in receipts; isolated credentials/config do not replace the user's CLI state. A mock, model list or login page cannot satisfy this gate.

**Validation:** real authenticated integration plus transport unit tests and simulated malformed/error events. Consume only a few small turns; quota exhaustion is a recorded blocking condition, not permission to switch billing or run large retry loops.

**Out of scope:** feature planning UI, repository access, takeover, custom OAuth, remote transports, SDK wrapper integration. If subscription access fails, stop dependent AI work and revise the integration recommendation.

### M0-B: native editor, input and terminal feasibility

**Largest uncertainty:** whether GPUI Kit provides a strong enough foundation without becoming an editor-framework project.

Tasks:

1. Pin GPUI Kit 0.9.1/default GPUI snapshot and a compatible compiler. Build one window with two editor buffers and a small task-side panel. Exercise public editor change/provider hooks.
2. Check multiline edits, grouped undo/redo, large paste, tabs, selection, clipboard, find/replace, IME composition, dead keys, emoji and combined characters; compare resulting bytes after save/reload.
3. Attach one actual rust-analyzer session for unsaved diagnostics, completion and definition. Distinguish this from the upstream example's simulated results.
4. Connect portable-pty to an alacritty_terminal grid rendered in a minimal GPUI view. Exercise shell prompt, Unicode output, resize, Ctrl-C and child cleanup.
5. Check keyboard focus, VoiceOver identification/navigation, zoom/contrast and responsiveness while model-like streaming and terminal output are simulated.

**Acceptance:** no observed text loss or undo corruption on the fixture set; usable composition and keyboard flow; VoiceOver can reach editor/task/actions and expose source text; actual LSP response is mapped correctly; terminal completes the basic process lifecycle. Proposed performance target on the user's machine: p95 input-to-frame below 50 ms for a representative 20,000-line file, with no UI stalls above 250 ms in the measured scenario. Record measurement method and hardware; these are targets, not current facts.

**Validation:** focused editor/provider fixtures plus manual input/accessibility checks and a bounded recorded performance run. Do not claim an automated test establishes comfortable daily typing.

**Out of scope:** polished appearance, exhaustive terminal escape support, broad language support and a framework fork. Bound the spike to two focused development days before reassessment; report a failure rather than extend indefinitely. If repair requires owning a substantial editor fork, recommend the Tauri/Monaco fallback and seek architecture approval.

### M0-C: execution and context isolation

**Largest uncertainty:** whether the pinned permission profiles enforce the needed boundaries across every available executable tool path.

Tasks:

1. Define companion, validation and takeover profiles using pinned schema/source. Limit experimental API usage to necessary profile selection; produce an explicit allowed-tool inventory.
2. With disposable sentinel files, attempt direct and subprocess reads/writes outside allowed roots, symlink escapes, path traversal, credential-like reads, unexpected tool calls and tool network access. Use controlled test data, never actual secrets.
3. Verify companion cannot modify live buffers/source; validation can write declared scratch/artifact locations; takeover can edit only selected scratch source paths. Check out-of-scope writes are denied even if the model requests them.
4. Verify build-script/test behavior and cache access, cancellation/process-tree cleanup, and `command/exec` operation without an authenticated model connection.
5. Test configuration/skills/plugin/instruction isolation, no cross-project memory, and no implicit authority from hostile repository text. Ensure unsupported policy shapes fail closed.

**Acceptance:** original-workspace and denied sentinels unchanged; unauthorized contents absent from tool output; allowed checks can complete; blocked checks report limits accurately; no unexpected network/tool access or lingering writable process. The tested boundary remains enforced with model instructions attempting to override it. No UI label or prompt-only test is sufficient.

**Validation:** deterministic policy tests, real macOS sandbox/process probes, and a small real-model attempt against harmless fixtures if needed. Store sanitized receipts. Re-run the subset on every supported Codex update.

**Out of scope:** building a new universal sandbox, Windows parity or allowing full host access to make a failing test pass. Failure requires a revised architecture/policy recommendation before integration proceeds.

## M1 — A dependable local editor

**User-visible result:** open and edit a Rust project with no AI connection required.

**Scope/dependencies:** M0-B passed; basic application shell, buffer lifecycle and local recovery. Initial module structure follows the architecture, without unused service/plugin layers.

**Largest uncertainty:** reliable file and buffer behavior under external changes and crashes.

**Tasks:** folder open/recent projects; file tree/quick-open; tabs and editor commands; Rust/Python highlighting; find/replace/project search; UTF-8/newline handling; safe saving/conflict UI; watcher reconciliation; settings persistence; dirty-buffer snapshots; native menus/shortcuts and basic diff view. Wire the proven terminal slice with bounded scrollback and cleanup.

**Acceptance:** edit/save/reopen/undo works across multiple files; restart offers unsaved recovery without overwriting disk changes; read-only/unsupported files fail clearly; terminal runs a local command and closes its child; large/binary files get a deliberate limit rather than hanging. A project can be used without sign-in.

**Validation:** focused IO/conflict/recovery fixtures and a manual 30-minute coding session, including external edits and forced termination. Check an app bundle launch early so GUI PATH/resource problems do not wait until M5.

**Out of scope:** AI tasks, persistent cross-restart undo stacks, multiple windows, debugger and advanced Git operations.

## M2 — Language tools, validation and durable task state

**User-visible result:** Rust diagnostics/navigation/completion, executable checks with trustworthy receipts, and task state that survives restart.

**Scope/dependencies:** M1 and M0-C; no model-driven source changes.

**Largest uncertainty:** mapping unsaved code into language tools and isolated validation accurately.

**Tasks:** bounded LSP client, rust-analyzer discovery/health, versioned document sync and provider bindings; explicit project-tool trust; diagnostic links and formatting action; validation recipes and artifact roots; snapshot materialization and path mapping; SQLite task/plan/step/check schemas; migration/recovery journal; runtime health/status integration.

**Acceptance:** completion/hover/definition and diagnostics reflect unsaved code; stale responses are ignored; a test runs on the captured unsaved snapshot, with output mapped back to original paths; missing dependencies are reported as unvalidated; original source and unrelated files are untouched; local checks work without AI. A failed server does not break editing.

**Validation:** real LSP/check integration on a small Rust project; Unicode/version fixtures; simulated server failures; crash/restart during snapshot and check. Review the recipe envelope once, then repeat checks without redundant prompts.

**Out of scope:** broad LSP extensions, automatic installation, general plugin framework and AI review.

## M3 — The companion loop, with Rust and Python

**User-visible result:** first companion preview: analyze a feature, revise a plan, implement guided steps, ask questions, reveal optional examples, review and validate.

**Scope/dependencies:** M0-A and M2. Add Python/Pyright and interpreter/test selection before declaring the workflow validated across languages.

**Largest uncertainty:** useful task guidance and evidence-based reviews without unwanted solutions or stale conclusions.

**Tasks:** context manifest/deltas; task decomposition and acceptance schemas; step UI; per-field settings/provenance; hidden/revealed solution channels; task questions; explicit review; evidence-backed findings; review applicability; Python tool adapter; connection/auth/quota states and task resumption. Disable memory generation/use in the isolated Codex profile.

**Acceptance:** complete one small Rust feature and one Python feature through analysis → plan → manual implementation → review → validation; mixed-language preferences resolve correctly; a valid alternative is accepted; a faulty implementation receives a specific justified next action; reveal makes no editor or permission changes; concurrent typing makes a review stale; connection failure preserves work. The user can change an approach or help level without starting a new task.

**Validation:** deterministic settings/state tests, seeded simulated results, small real subscription-turn fixtures and user-facing manual journeys. Separate backend output quality from application rendering/policy correctness. No AI calls on each keystroke.

**Out of scope:** takeover, benchmark automation, educational assessment and inferred learner profiles.

## M4 — Scoped takeover and code improvement

**User-visible result:** deliberately delegate a step/feature, inspect and safely import its changes, and request clarity/speed/balanced improvement proposals.

**Scope/dependencies:** M3 and proven isolation. One foreground agent run; optional checks stay within the approved grant.

**Largest uncertainty:** safe import/undo with pre-existing and simultaneous user edits.

**Tasks:** grant/scope panel; materialized baseline with dirty/untracked inputs; takeover run lifecycle; streamed activity/diff; source/artifact scope enforcement; version-checked import as an undo group; three-way conflicts and inverse operations; cancel/handoff; optimization result schema; benchmark recipe proposals and measured-versus-expected labels.

**Acceptance:** take over a multi-file feature without changing the live tree until import; preserve unrelated dirty/untracked changes; deny out-of-scope writes; block overlapping imports; undo retains later user edits or surfaces a conflict. Interrupt during a check and retain an honest activity receipt. A benchmark-backed suggestion preserves behavior and displays practical/readability costs; a speculative suggestion never displays as measured.

**Validation:** deterministic change/import journals, adversarial sandbox fixtures, simulated cancellation/reconnect races and a bounded real takeover. Use sorted-search and allocation examples with explicit preconditions; include a case where the best recommendation is to keep the existing implementation.

**Out of scope:** autonomous commits/deployments, parallel agents, live shared-buffer editing and automatic application of improvement suggestions in companion mode.

## M5 — First usable macOS release

**User-visible result:** install and launch Tandem locally as a standalone `.app`; complete the normal Rust/Python coding journeys without launching a development server.

**Scope/dependencies:** M1–M4. Stabilize, package and verify; add only missing first-release essentials from the product matrix.

**Largest uncertainty:** real daily usability and process/resource behavior outside the development shell.

**Tasks:** bundle compatible Codex/resources; resource-relative executable discovery; explicit toolchain paths and health screen; recovery UX; keyboard/focus/VoiceOver fixes; output/backpressure limits; license inventory/third-party notices; versioned local release directory and rollback; install receipt and concise user instructions. Keep signing and notarization as an explicit distribution step if moving beyond this Mac.

**Acceptance:** launch from Finder; open project, edit, run diagnostics/checks/terminal, complete companion and takeover flows, quit/restart and recover; operate offline; gracefully handle quota/auth/process failure; no source loss or leaked children in the tested scenarios. The previous local release remains available. Report actual known limitations instead of calling untested platforms supported.

**Validation:** full meaningful verification matrix, a 60-minute local coding session, clean-environment app launch, process cleanup and license/package inspection. Repeat passed checks only for changed components or unresolved risk. Distinguish agent testing from the user's usability feedback.

**Out of scope:** public release infrastructure, updater service, Windows and IDE feature parity.

## M6 — Develop Tandem inside Tandem

**User-visible result:** Tandem is useful for its own normal Rust development while a known-good build remains available.

**Scope/dependencies:** M5. This milestone uses real changes in Tandem, not a demonstration repository.

**Largest uncertainty:** whether the workflow holds up on a larger, evolving application with native dependencies and frequent restarts.

**Tasks:** open Tandem's source; configure its real build/test recipes; build a distinct development bundle from the stable running release; complete two bounded changes (for example, a task-panel behavior and a persistence/recovery fix); use companion guidance for one user-selected manual task and takeover for one explicitly delegated task. Codex can carry the engineering work through; the user is not required to author changes to unblock the build.

**Acceptance:** navigate/edit real Rust modules, inspect diagnostics, run meaningful tests, inspect/import a takeover diff and launch the new build from within Tandem's workflow; resume after rebuilding/restarting; use an independent launcher/previous bundle to recover from a broken dev build. No external editor is needed for the exercised normal path, though it remains a recovery option.

**Validation:** record changes, checks and user feedback from at least two separate development sessions; fix the highest-impact workflow failures before expanding languages or platforms. This proves a bounded self-development path, not total replacement of every existing tool.

**Out of scope:** a self-hosting compiler/toolchain, every possible repository setup and mandatory user participation in implementation.

## Later, only after the core is useful

Add Go, then JavaScript/TypeScript, then Java using explicit tool/runtime acceptance fixtures. Evaluate more editor features from actual use. Windows needs its own rendering/input, filesystem, process, credential, sandbox and installer gates. Public distribution requires signing/notarization and a tested update/recovery story. Do not prebuild those systems into v0.1.
