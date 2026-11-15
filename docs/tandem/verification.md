# Verification strategy

Status: mixed evidence, with unverified gates retained. [Plan entry point](README.md). The guided-companion Task 10 deterministic/simulated journeys, bounded real results and blocked native checklist are recorded in [guided-companion.md](guided-companion.md). Those results do not mark every scenario below or any full milestone passed.

Use focused tests for meaningful behavior. Test domain rules deterministically; inject backend errors at adapter boundaries; use real Codex/LSP/process integrations where simulation cannot establish the claim; manually check typing and accessibility. Do not test a prompt by merely comparing it with a copy of itself, and do not use model confidence as validation evidence.

## Evidence classes

| Class | Establishes | Does not establish |
| --- | --- | --- |
| D — deterministic | Settings precedence, revisions, policy decisions, imports, persistence and rendering invariants | Real authentication, OS sandbox enforcement or model quality |
| S — simulated integration | Error ordering, duplicate events, malformed data, disconnects, cancellation races and quota UX | That the real service produces identical timing or supports a field |
| R — real integration | Pinned runtime/auth/LSP/process behavior on recorded environment | Broad platform support or consistent model performance on all tasks |
| M — manual usability | Actual input methods, keyboard flow, VoiceOver and useful coding journeys | Exhaustive correctness or security |

Each receipt records fixture, code/runtime versions, OS/hardware where relevant, expected behavior, actual result and unresolved limitations. Redact credentials and private source. Tests that need billing/auth are explicit opt-in suites; normal deterministic tests do not consume subscription quota.

## Required scenarios

| ID / gate | Scenario and decisive assertion | Class |
| --- | --- | --- |
| V01 / M0-A | With API-key/provider overrides absent and Tandem's isolated home selected, managed ChatGPT login leads to a real nonempty successful turn. Follow-up depends on prior content; restart/resume completes another turn. Record auth mode, final status, binary/schema IDs and sanitized response. Login/model listing alone fails this test | R |
| V02 / M0-A,C | Private home/config/Keychain behavior does not replace unrelated CLI authentication/settings. No token/login URL in logs/UI/database. API-key account state is rejected rather than silently used | D, S, R |
| V03 / M0-C | Pinned profile selection works; old restricted-read shape is rejected; unknown permission requests fail closed. Direct/subprocess/symlink/path traversal attempts cannot read denied sentinel content or write original source. Tool network denied; allowed model transport remains functional | D, R |
| V04 / M3 | Mixed Rust/Python/JS task resolves every field through global → language → project → project-language → temporary step layers. Removing an override inherits anew; language identity follows guided symbol. Provenance is correct | D |
| V05 / M3 | Hidden solution is absent from visible/accessible tree, task summaries, search, notifications and streamed interim content. An injected unexpected solution field is quarantined. Detailed explanation does not invoke solution generation. Reveal for Rust leaves Python hidden | D, S, R, M |
| V06 / M3 | Reveal, explain, copy and automatic preview change neither grant nor buffer/source hash. Attempting an AI edit after reveal remains denied by broker and real runtime | D, R |
| V07 / M3 | Python fixture implements the same contract using a different valid algorithm/structure from the optional example. Review accepts it against criteria, tests and caller semantics. No reference-solution string comparison exists | D, R |
| V08 / M3 | Rust/Python fixtures with empty-input, boundary and interface failures yield specific findings with criterion/evidence, why it matters, smallest next action and blocking status. Unsupported claims are labelled concerns; style remains optional | D, S, R |
| V09 / M3 | Begin review on revision N, then edit relevant buffer/dependency/criteria before completion. Result remains attached to N, cannot mark N+1 complete or show current error decorations. Changes with uncertain dependency relevance conservatively invalidate | D, S |
| V10 / M2,M3 | Review/validation uses unsaved overlay; check receipts name snapshot and tool versions. A model's claimed test pass without a receipt remains unverified. Missing environment/dependency is not reported as a correctness failure or successful validation | D, S, R |
| V11 / M4 | Sorted-search proposal preserves duplicate/range/empty semantics; unsorted, linked-access or tiny/one-shot cases prevent a mechanical binary-search recommendation. No benchmark means “expected,” never “measured.” Actual benchmark records build mode, dataset, frequency, variance and tradeoffs | D, R, M |
| V12 / M4 | Takeover baseline includes existing dirty, untracked and unsaved work. Approved scratch edits succeed; out-of-scope and live-root writes fail. Import touches only authorized buffers; failed checks are visible and never treated as passed | D, R |
| V13 / M4 | User edits during takeover: disjoint changes survive; overlap blocks import with base/current/agent versions retained. Undo after additional user edits preserves them or opens a conflict. No stash/reset/whole-tree restore | D, M |
| V14 / M4 | Interrupt while a command starts/finishes. Stop dispatch, terminate/reconcile child processes, keep completed effects and honest unknown states. Returning control does not auto-import; already completed commands are not described as undone | D, S, R |
| V15 / M1,M5 | Kill app during dirty recovery, save, SQLite transaction and import phases. On restart recover journal/content or identify incomplete state. Disk divergence produces choices, never silent overwrite. Bound/document the debounce recovery window | D, S, R |
| V16 / M3,M5 | Simulate unauthorized, quota exhaustion, model unavailable, server exit and lost completion response. Preserve draft/task/buffers, reconcile before retry, do not duplicate mutation or switch billing. Simulate quota rather than exhaust the user's subscription | S, R for ordinary disconnect |
| V17 / M1,M2,M5 | Start with AI unavailable: open/edit/save/undo/search/terminal and installed LSP still work. Local validation uses its tested independent execution path. UI never requires an AI heartbeat to process edits | D, R, M |
| V18 / M0-B,M5 | IME/dead keys/emoji/combined characters, clipboard, multiline edits, CRLF/BOM and multi-file undo/save round trips preserve text. Keyboard navigation and VoiceOver can reach all core actions without focus theft | D, M |
| V19 / M2 | Real rust-analyzer and Pyright on versioned fixtures: unsaved diagnostics, completion, definition, references; Unicode offsets; capability negotiation; stale results and child restart. No proprietary editor-only server assumption | D, S, R |
| V20 / M2,M4 | Validation recipe writes artifacts only where declared, cannot mutate original source, captures stdout/stderr/exit and cleans child tree. Legitimate generated-source writes require a declared copied location. Shell-metacharacter strings cannot escape argv dispatch | D, R |
| V21 / M3 | Hostile repo instructions/tool output cannot alter settings, request hidden answers into visible fields, enable plugins, reveal sentinels or grant writes. Ignore/secret/size rules are enforced during materialization and context expansion | D, S, R |
| V22 / M5 | Finder-launched packaged app finds resources/runtime, handles missing language tools clearly, preserves previous release and ships notices. Test actual macOS target; report Intel/Windows untested | R, M |
| V23 / M6 | Two real Tandem changes are developed with its editor, checks, task flow and diff/import UI; a failed dev build can be recovered using the previous stable bundle | R, M |

## Fixture design

Use small original repositories: Rust library with a caller and edge-case tests; Python package with pytest and unittest variants; mixed-language project; dirty Git tree; plain non-Git folder; Unicode/CRLF files; controlled symlinks; generated files; ignored/secret-like sentinels; and a large text fixture. Do not use real credentials or destructive host commands. Keep sandbox probes inside disposable app-owned directories with explicit expected allowed/denied paths.

For model-quality cases, establish acceptance criteria and executable evidence independently of the suggested implementation. Include at least two valid alternatives and one incorrect variant per core behavior. Evaluate a small repeat set under a recorded available subscription model, report observed pass/failure counts and examples, and do not extrapolate a broad quality score. Never store those outcomes as measurements of the user's ability.

For stale reviews, vary buffer changes, dependency changes, external writes, settings changes, criteria revisions and response ordering independently. Verify both content applicability and UI labels. A tracked text decoration moving with an edit does not make its finding current.

For optimization, use both a correctness oracle and representative benchmarks. Include preprocessing/sorting cost when relevant, warm/cold behavior where meaningful, repeated samples and outliers. A faster microbenchmark is not automatically a faster application. Preserve slower/readable alternatives when the tradeoff is optional.

## Release gates and limits

M0 requires real auth and real boundary evidence; simulated success cannot substitute. M3 requires usable Rust and Python manual coding journeys plus specific real-model examples. M4 requires deterministic import/undo protection and real isolation. M5 requires packaged offline editing/recovery and a manual accessibility/input pass. M6 requires real self-development receipts.

Do not mark a milestone complete with known source loss, overwrite, permission escape, billing fallback or inaccessible essential action. A dependency limitation can be an explicit scope revision, not an unreported exception. Re-run focused affected checks after changes; expand testing only for failures, new code or unresolved risk. The final report separates automated results, real integration results, agent usability checks and user feedback.
