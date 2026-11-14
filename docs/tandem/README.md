# Tandem plan

Status: approved by the user on 2026-11-13. M0–M5 are implemented and verified on the development Mac. M3 includes real subscription-backed Rust/Python journeys and native companion, Python, guidance and recovery checks. M5 packages the verified runtime, adds a versioned install/rollback path and closes release usability gaps; see [M5 results](m5-progress.md) and [local usage](local-use.md). M4 adds scoped takeover, draft import/undo and measured improvement proposals; see [M4 results](m4-progress.md). See [M3 results](m3-progress.md), [M2 results](m2-progress.md), [M1 results](m1-progress.md) and [M0 results](m0-progress.md). The original research below is not runtime validation.

## Recommended direction

Build a standalone, local macOS IDE in Rust, using **GPUI Kit's editor and UI components**, with **Codex app-server as a supervised child process over stdio**. Keep a task outline, current implementation step, guidance controls, and review evidence beside the code. Chat is available within that workspace. Companion mode is the default; execution authority is a separate, explicit choice.

Reuse editor primitives, syntax parsers, language servers, and a terminal engine. Build Tandem's workflow, persistence, context selection, and permission enforcement. Do not fork or rebrand an existing IDE.

This recommendation is conditional on early evidence, not an assertion that the components already work together. Before building the main application, prove a real subscription-backed turn, enforceable execution boundaries, and a usable native editor. If a gate fails, return with a revised recommendation. The preferred UI fallback is Tauri 2 with Monaco and a TypeScript frontend; that would be a material architecture change requiring approval.

## What the first usable version does

Open a local Rust or Python project; edit multiple files with reliable save/undo, search, syntax highlighting, LSP diagnostics/navigation/completion, command output, a basic terminal, and change views. Start a feature, examine its decomposition, implement a step with the solution hidden or available on request, and receive a version-specific review supported by diagnostics and checks. Explicitly delegate a step or feature into an isolated working copy, inspect its diff, import it without overwriting newer work, and return to companion mode. Resume local work after restart or loss of AI access.

An earlier companion preview arrives before takeover. The first usable release is complete at M5, including a locally installable Apple Silicon `.app`. M6 establishes developing Tandem inside Tandem. These are acceptance gates, not calendar promises.

## Read the plan

| Document | Purpose |
| --- | --- |
| [Product](product.md) | Requirements, defaults, scope, language sequencing, non-goals |
| [Architecture and research](architecture.md) | Ranked choices, evidence, runtime contract, state, security, reuse and licenses |
| [UX and interactions](ux.md) | Layouts, journeys, step lifecycle, settings, reviews, failure states |
| [Implementation roadmap](roadmap.md) | Ordered milestones, tasks, acceptance, validation and exclusions |
| [Verification strategy](verification.md) | Deterministic, simulated, real integration and manual checks |

## Approved direction

1. Rust-native application using GPUI Kit 0.9.1's pinned upstream GPUI backend; no web frontend or plugin platform in the initial design.
2. Direct Rust app-server client; begin compatibility work with installed Codex 0.169.0. Use Codex-managed ChatGPT browser login in a Tandem-specific Codex home. No API billing fallback.
3. Local operational state in SQLite; editor recovery snapshots outside the project. No learner model or cross-project AI memory.
4. Companion AI receives selected snapshots. Validation executes in isolated copies with declared artifact writes. Takeover never edits the live workspace directly; Tandem imports authorized changes with version checks.
5. Rust first, Python before the first usable release; Go and JavaScript/TypeScript next, Java afterward. No debugger, remote workspaces, extension marketplace, or full Git client in v0.1.

## Main risks and their gates

| Risk | Resolution |
| --- | --- |
| Login works but a subscription turn does not | M0-A completes a real turn and resumes it under ChatGPT authentication, with API credentials absent |
| Documentation differs from the installed protocol; sandbox grants are too broad | M0-A pins schemas; M0-C verifies denied reads/writes, subprocesses, symlinks, network and cancellation using the actual binary |
| Native editor quality consumes the project | M0-B checks input, undo, accessibility, LSP hooks, rendering and terminal integration before product work |
| Reviews judge a suggestion rather than behavior, or report stale results | M3/M4 fixtures accept alternatives; snapshots and acceptance revisions control result applicability |
| Takeover or undo discards user work | M4 uses isolated copies, baseline hashes, journaled import and three-way conflict handling |

No product or architecture question blocks the approved plan. M6 is next: develop Tandem inside Tandem while retaining a known-good local release. Signing credentials are only needed if distributing to other Macs; local installation comes first. A decision to require an entirely stable Codex permission API would change the integration plan: 0.169.0's explicit profile-selection fields are experimental.

## Evidence boundary

The directory was empty and was not a Git repository. No applicable `AGENTS.md` or documentation convention was found in the directory or checked ancestors. Local read-only inspection found Apple Silicon, macOS 27.0.1, Xcode 27.2, Codex CLI 0.169.0, Node 24.22.1, npm 12.0.1, and Rust/Cargo 1.95.0. Rust is installed under `~/.cargo/bin`, which this shell does not include in PATH. Codex reports ChatGPT login. No credentials or global configuration contents were read or copied.

During the initial planning phase, only Markdown planning documents were added. No Git initialization, application code, dependency installation, model turn, build, sandbox probe, or global configuration change was performed in that phase. The Rex session was named `tandem-planning`.
