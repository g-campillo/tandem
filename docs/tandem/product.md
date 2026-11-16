# Product specification

> **Historical.** This document describes the earlier Codex-based design, which was replaced on 2026-11-15 by the Claude chat companion. The current design is in [companion.md](companion.md).

Status: proposed. [Plan entry point](README.md).

## Purpose

Tandem helps an experienced engineer think through and write real software with an AI companion. It explains unfamiliar languages and concepts at the point of use, supports independent implementations, and evaluates behavior using evidence. It also offers deliberate delegation when speed matters.

The initial user has nearly a decade of engineering experience, strongest in Python, then Java, then JavaScript, and wants to learn Rust, Go, and TypeScript. Programming experience and language familiarity are separate. Tandem should never infer a need for elementary programming instruction from unfamiliarity with Rust ownership or a particular library.

Codex will build Tandem after plan approval. The guidance workflow describes how the user will use the finished application; it does not require the user to manually implement Tandem during this build.

## Product rules

| Requirement | Product consequence |
| --- | --- |
| Planning precedes work | Every requested task has an objective, relevant context, constraints, unknowns, acceptance criteria and proposed steps; a small fix can have one step |
| The user normally writes code | Companion mode exposes proposals, questions and reviews; no AI source-write capability |
| Help is configurable | Instruction detail, explanation depth and solution visibility are independent of execution authority |
| Alternative implementations are welcome | Reviews use acceptance criteria, integration contracts and evidence; no textual answer matching |
| Subscription access is required | Codex manages ChatGPT authentication; no API-key field or automatic paid fallback |
| Editing survives AI failure | Open/save/undo/navigation/local diagnostics and human terminal remain usable offline |
| This is a personal desktop application | Local files and state; no Tandem account, server, collaboration service or billing system |
| Learning is incidental to real work | Persist task decisions and settings, never scores, weaknesses, curriculum, streaks or learning profiles |
| Authority is visible | A persistent mode indicator describes permitted actions; reveal, copy and explain never authorize application |

## First usable release: v0.1

One local project per window, initially one window and one foreground AI operation. Multiple saved tasks can be resumed; one is active. Plain folders and Git repositories both work. Git is optional for editing and isolated-copy takeover.

| Capability | v0.1 commitment | Later |
| --- | --- | --- |
| Project access | Folder picker, recent projects, file tree, quick-open; explicit local project trust for executable tooling | Multi-root workspaces, remote/container projects |
| Editing | UTF-8 including BOM and CRLF preservation, tabs, selections, clipboard, reliable undo/redo, atomic save, dirty indicators, external-change conflicts | Broad legacy encodings, collaborative editing, Vim emulation |
| Navigation/search | Current-file find/replace; project text search and quick-open; definition/references and symbol outline where LSP supports them | Cross-project indexing, semantic search |
| Language tools | Rust and Python highlighting, diagnostics, hover, definition, ordinary symbol completion; explicit formatting and safe code actions | Debug adapters, exhaustive LSP coverage |
| Commands | Registered validation recipes, streaming output, exit status, cancellation, diagnostic links | General workflow graph/task automation |
| Terminal | One local shell PTY pane, resize, scrollback, selection/copy/paste and Ctrl-C; clearly human-controlled | Multiplexing, remote sessions, image protocols |
| Changes | Disk-versus-buffer, Git worktree status/diff if available, takeover diff and undo | Stage/commit/rebase UI, merge management beyond import conflicts |
| Companion | Analyze, editable plan, meaningful steps, hints/explanations, optional solution, questions, review and validation | Background multi-task agents |
| Improvement | Explicit clarity/speed/balanced review; small, evidence-labelled proposals | Automated benchmark laboratory |
| Takeover | Step or feature scope; isolated execution; reviewable import; safe cancellation and handoff | Live simultaneous co-editing, autonomous deployment |
| Accessibility | Keyboard reachability, visible focus, scalable text, contrast, VoiceOver basics and no typing interruptions | Full accessibility audit across platforms |
| Recovery | Open files/layout, dirty-buffer recovery, tasks/settings/reviews and interrupted operations | Persistent terminal processes and undo history across restarts |
| Delivery | Locally installable Apple Silicon macOS application, runtime health screen and bundled compatible Codex executable | Signed/notarized public distribution, Intel and Windows |

The first editor preview is M1. The first companion preview is M3. v0.1 requires M5, rather than treating an attractive chat demo as a usable IDE.

## Defaults

- Start in companion mode. Start with hidden solutions, standard step instructions and concise explanations. Offer optional Rust and Python presets at project opening; do not silently attach abilities to languages.
- Suggested Python preset: high-level instructions, concise explanation, hidden solution. Suggested Rust preset: detailed instructions, detailed explanation, solution available automatically in a separate preview only if that preset is explicitly chosen. Either preset can be changed per field.
- Keep traditional LSP completion available. Disable AI inline implementation completion in v0.1. Formatting is an explicit conventional editor action, with undo, and does not constitute AI takeover.
- Run cheap editor diagnostics at language-server boundaries. Do not run model reviews on keystrokes. Default AI review trigger: **Review this step**. Optional after-save review requires the step to be marked ready, a quiet interval and one review per changed snapshot.
- Saving never silently reformats or starts a model turn. Validation recipes can be authorized once per project/recipe revision; prompts recur only for changed authority, not repeated checks within the approved scope.
- Proactive performance findings occur during requested reviews and only for concrete, relevant problems. No background optimization stream.
- Keep full solutions out of task summaries, search snippets, notifications and hidden-step accessible text. Generate a solution only after reveal or an explicit automatic-preview preference. Once seen, collapse is possible; it cannot undo having seen it.

## Core journeys

1. **Open and orient:** open a folder; edit immediately; inspect language/tool availability and effective assistance settings; optionally sign in. Trust enables local executable language/build tools, not autonomous editing.
2. **Start a feature:** describe the outcome; the companion inspects selected context, states assumptions/unknowns, proposes acceptance criteria and a short dependency-aware plan. The user edits or accepts it.
3. **Implement:** choose a step; read responsibility, rationale, affected symbols and done conditions; ask for a hint or explanation; write an implementation without a forced solution or quiz.
4. **Review and validate:** capture the relevant buffer versions; inspect specific findings and evidence; accept a valid alternative; run the appropriate recipe; complete the step or revise the plan.
5. **Improve:** choose clarity, speed or balanced review for a symbol/file/change. Receive behavior-preserving options, tradeoffs and a benchmark proposal when measurement matters.
6. **Delegate:** select a step/feature, review scope and allowed checks, start takeover; observe activity and isolated diffs; cancel or take control; inspect and import changes; continue manually.
7. **Resume:** reopen the project with dirty buffers and task position restored. Interrupted work is visibly incomplete. Reconnect AI without blocking the editor or silently rerunning commands.

## Guidance is not assessment

Settings are explicit user preferences. The application does not infer proficiency from errors, review history, language choice, help requests or time spent. A completed step is operational task state, not educational achievement. Conversation summaries preserve project decisions and unresolved work, not claims about the user's strengths or weaknesses. Disable Codex memory generation/use in Tandem's isolated profile; retain only task-scoped conversation continuity.

## Language sequencing

1. **Rust (M1–M3):** validate native editor integration and ownership/error-handling explanations on real code; rust-analyzer plus Cargo checks/tests. Build scripts and procedural macros execute code and belong to project trust and validation policy.
2. **Python (M3):** validate high-level guidance and acceptance of different implementations; Pyright LSP, selected interpreter/virtualenv, pytest or unittest recipes. Pyright introduces Node as a tooling dependency, not an application frontend runtime. Do not use proprietary Pylance as a generic embeddable server.
3. **Go, then JavaScript/TypeScript (after M6):** gopls and TypeScript language tooling through the same LSP boundary; document Go and Node toolchain requirements and add multi-root/package-resolution fixtures. Rust/Python remain release gates; no claim that highlighting alone is language support.
4. **Java:** Eclipse JDT LS, JDK discovery and Maven/Gradle workspace lifecycle. Defer its heavier project-import and runtime requirements. Audit each server's distribution/license before bundling it.

## Explicit non-goals

No learning tracking, mandatory quizzes, course content, teams, monetization, hosted accounts, cloud backend, multi-agent orchestration, universal extension API, VS Code extension compatibility, mature-IDE feature parity, remote development, autonomous publishing, automatic commits, or broad Windows support in v0.1. No custom model training or embedding database. No automatic conversion of ordinary ChatGPT API credits or other payment modes into a substitute for subscription-backed Codex.

## Decisions that remain open

The exact macOS minimum version follows the native dependency and input gate; initially validate on the observed macOS 27.0.1 machine. Older macOS and Intel are unclaimed until tested. Select compatible patch versions and a Rust toolchain after approval, without changing the user's global default. Public distribution and Tandem's own license can wait; dependency notices cannot. A change from native UI, a relaxed source-protection boundary, or any paid API requirement needs a revised plan and approval.
