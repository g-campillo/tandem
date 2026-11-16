# M4 — scoped takeover and code improvement

> **Historical.** This document describes the earlier Codex-based design, which was replaced on 2026-11-15 by the Claude chat companion. The current design is in [companion.md](companion.md).

Status: implemented and verified on the development Mac. Continuation was authorized after M3. Rust-native UI, subscription authentication and the default companion workflow are unchanged.

## Working plan

1. Define a durable delegation grant: accepted task/plan revision, step or feature, exact relative source paths, create/modify operations, allowed check recipes, network off, resource/round limits and captured working baseline. Include live buffers, retained closed drafts and untracked files. Refuse excluded, escaping or ambiguous paths.
2. Use **app-mediated structured edits and checks**. The pinned Codex app-server supplies edit batches and requested recipe IDs; Tandem validates/applies them only in an isolated working copy. Model OS/editing tools stay disabled. This keeps command authority in the existing recipe broker and makes path/recipe enforcement independent of prompts. Unknown or broader requests do not gain authority.
3. Run bounded implement → check → repair rounds within the grant, with visible activity, current staged diff, check receipts, stop control and handoff. Stop prevents further dispatch; completed writes/checks remain recorded. Restart marks unfinished work interrupted and never resumes delegation automatically. No commits, deployments, dependency installation or network expansion.
4. Import reviewed changes as a journaled group of **unsaved buffers**, leaving disk saves explicit. Compare baseline, staged and current working text; automatically merge only non-overlapping line edits. Preserve conflicting versions and block the affected import. The import journal and draft transaction must commit before updating the visible buffers.
5. Add **Undo agent import** using the same three-way machinery with inverse changes. Preserve later unrelated edits. An overlapping edit, externally changed disk baseline, or a created file subsequently saved to disk is a surfaced conflict, never a silent deletion/reset. The initial grant supports text-file creation/modification; deletion, rename and mode changes are outside this milestone's automatic edit surface.
6. Add clarity, speed and balanced improvement requests. Bind proposals to code/plan/tool versions and record invariants, preconditions, expected complexity/practical costs, readability/memory tradeoffs and validation gaps. Suggestions remain proposals. Benchmark execution uses an explicitly reviewed, bounded recipe and versioned receipt; “measured” requires actual comparable benchmark evidence, not model confidence.
7. Validate deterministic merge/import/undo/recovery cases; simulated malformed/out-of-scope agent output; real subscription-backed multi-file delegation; cancellation during a check; original/dirty/unrelated work preservation; and a representative benchmark with behavior checks. Exercise the native M4 controls in a separate development bundle/profile, retaining M3 as a known-good build.

## Acceptance boundaries

- Revealing guidance and granting delegation remain separate. Default companion requests cannot reach the mutation broker.
- A delegation can implement and repair a bounded multi-file change without any source edit in the live project. The user can keep typing; import rechecks current buffers and disk baselines.
- Every batch is checked against the immutable grant and current staged hashes. Out-of-scope changes/commands are rejected before dispatch; no raw model-generated shell command is executed.
- Import and undo preserve unrelated work, including pre-existing dirty/untracked content. Ambiguous/overlapping edits are conflicts. Filesystem-wide compare-and-swap against unrelated external writers is not promised; imports remain drafts and ordinary saves retain M1 conflict protection.
- Cancellation/restart preserves a useful handoff and any completed work. Unknown outcomes remain unknown; there is no automatic replay.
- Clarity/speed proposals do not mechanically apply optimization rules. A no-change recommendation is valid. Expected and measured results are distinct, with workload, preconditions, method, build/interpreter and correctness evidence attached.

The first experiment after these domain boundaries are implemented will exercise a real structured edit batch under ChatGPT subscription authentication, followed by the existing isolated check broker. Final claims will distinguish deterministic tests, real runtime behavior and native UI observations.

## Implemented result

M4 is implemented and verified on the development Mac. `tandem-agent` brokers structured create/modify batches from the existing subscription runtime; it does not enable model shell or edit tools. The native **Takeover** panel prepares a working snapshot, shows the immutable file/recipe grant, starts one bounded run, reports activity, offers captured staged diffs and returns control on stop. Accepted plans are prerequisites. Grant fields edited after preparation require a new prepared scope.

Source and validation copies have separate sibling cache roots. Commands run against another read-only snapshot, with both the live project and staging source denied; only the command artifact directory is writable. Exact executable/resource fingerprints must still match the selected recipes. A failed or unavailable check does not become a pass, broaden permissions, or automatically complete a step.

**Import** previews a three-way merge and commits the draft changes plus import journal in one SQLite transaction before updating the editor. Confirmation recomputes the preview; new edits require review again. **Undo Agent Import** computes an inverse merge, retaining later unrelated edits. Overlaps, disk divergence, changed created files, and created files already saved to disk block unsafe undo. A delegation cannot be reimported under a new journal ID. Schema 4 migration backs up earlier databases; active runs, including the authorization-before-dispatch window, recover as interrupted without replay.

**Clarity**, **Speed**, and **Balanced** request structured proposals from the current companion context. They separate invariants, preconditions, complexity, practical costs, tradeoffs, proposed steps, validation gaps and benchmark plans. Neither proposals nor revealed solutions authorize edits. Actual cited benchmark receipts are displayed separately from expected complexity; historical context remains labeled historical. A keep-current recommendation is supported and exercised.

### First comparison recipe

The **Python comparison benchmark** uses `benchmarks/tandem_compare.py` with `cases()`, `baseline(case)` and `candidate(case)`. Cases are 1–12 `(label, input)` tuples. A fixed application-owned harness deep-copies each input outside the timed interval, alternates callback order, performs two warmups and seven measured samples, and checks returned values for equality on every iteration. Receipts record per-case raw nanoseconds, min/median/max, interpreter, source snapshot, method and harness fingerprint. Execution has the existing 120-second and output bounds, no tool network and no source writes.

This measures the supplied callbacks on named cases, not universal equivalence or end-to-end workload cost. Setup/copying/sorting is excluded unless the callback does it. The callbacks are trusted project code, like tests; this is not an anti-tampering benchmark service. Rust benchmark frameworks and arbitrary benchmark commands are deliberately not added to this initial recipe.

## Verification actually performed

| Category | Observed result |
| --- | --- |
| Deterministic/workspace | **64 passed**, zero failed. Includes scope/recipe rejection, ambiguous replacement refusal, normalized path collisions, staged context tampering, disk/symlink conflicts, non-overlapping merge, overlap refusal, created-file undo constraints, atomic journals, replay prevention, interrupted grants and measurement-shape checks |
| Real subscription delegation | One round implemented three Rust files, including a new helper, and passed the explicitly approved Cargo tests. Original scoped files stayed unchanged; a pre-existing dirty note and unrelated draft survived import and undo |
| Real native takeover | Prepared and authorized exact paths plus Cargo test. Corrected staging placement reached the sandboxed command. ⌘. during a deliberately slow test produced **Cancelled** for the check and takeover; completed staged edits remained inspectable and live source stayed unchanged |
| Native import/recovery | Imported three unsaved buffers; preserved a pre-existing draft and a later non-overlapping edit; added another user edit, quit/restarted, recovered three drafts and the journal, and undid the import while preserving both user edits |
| Real benchmark | Empty, duplicates/first-index, misses and 128-query workloads at n=32/1,024/16,384 passed returned-output comparisons. A deliberately incorrect candidate failed without measurement evidence; a slow benchmark cancelled without evidence or source writes |
| Real improvement proposals | Balanced proposal cited the actual benchmark and limited its recommendation to sorted random-access repeated lookup. Separate Speed request recommended **KeepCurrent** for one tiny unsorted heterogeneous lookup, including allocation/sorting/index costs, with no measured claim or edits |
| Native proposal display | Retained real Python result displayed theoretical complexity separately from receipt measurements, stated preconditions and tradeoffs, preserved hidden-solution policy, and showed no edit permission |
| Build/package | Formatting, strict workspace/all-target Clippy, development build and ad hoc signature verification passed. M4 has its own bundle and QA profiles; earlier bundles/profiles remain available |

The ordinary test run has two intentionally ignored entries: the crash-writer helper is invoked by its parent test; the real transport-loss regression was exercised in M3 and was not repeated for M4 because transport did not change. Real runtime and model experiments above ran separately from unit tests. No new user typing/VoiceOver session, 60-minute coding session, clean-Mac distribution test or Windows check is claimed.

Issues found and resolved during this milestone:

- The first concurrent import fixture inserted at the same position as the agent. The merge correctly refused it. The retained actual result then passed a genuinely non-overlapping edit and inverse merge; conflict detection was not weakened.
- Undoing a new nested draft initially treated its absent parent directory as a disk error. Recovery/import now recognize absent unsaved paths while retaining symlink and disk-conflict checks. Explicit Save creates validated parent directories.
- Native staging initially lived under the validation-run cache. The existing permission overlap guard rejected the command as **Unvalidated** with `Choose a repository folder that does not overlap toolchain, cache or runtime directories`. Staging now uses a sibling root; native validation reached the test process and cancellation passed.

Sanitized evidence: [verification record](evidence/m4/verification.json). Detailed fixture receipts and logs remain under `.tandem-local/m4/`; credentials, auth URLs and raw protocol streams are not copied into documentation.

## Use and limits

- Open **Takeover** with ⌥⌘T. Select step or feature, enter exact paths if the plan does not provide them, choose checks, prepare, review, then authorize. Editing remains available during takeover. ⌘. stops AI work. Inspect a captured staged diff at any point; import is only available after a terminal handoff.
- **Import Reviewed Changes to Buffers** leaves saves explicit. ⌥⌘Z previews the latest applied import's inverse; confirm it to change buffers. Ordinary editor undo remains separate. Check receipts from staging are evidence about that snapshot, so run checks again after a merge that includes additional user changes.
- Limits: one foreground agent, 1–16 exact source paths, 256 KiB scoped/batch source, at most four rounds. Text create/modify only; no deletion, rename, executable-mode change, automatic commit, deployment or dependency installation. Conservative line merges intentionally surface same-line/ambiguous insertion conflicts.
- Review and import may read the bounded snapshot synchronously; large-project responsiveness and cache retention controls remain M5 hardening. There is no automatic staging cleanup or automatic delegation resume. Historical runs and journals are retained locally. Unsent form fields remain unpersisted.
- New dependency `unicode-normalization` **0.1.26** implements NFC path collision checks. Its installed crate manifest declares **(MIT OR Apache-2.0) AND Unicode-3.0**; preserve the selected software license and Unicode notice in M5's complete distribution inventory. No upstream code was modified.

**Next: M5**, bundle/runtime discovery, license notices, installation/rollback and a sustained local coding session. M6 remains the explicit milestone for developing Tandem inside Tandem.
