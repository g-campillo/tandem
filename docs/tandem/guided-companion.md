# Guided companion verification

A task begins as a persistent discussion. Research receipts inform separate Spec and Plan proposals; each document must be saved and explicitly approved. Start Implementation selects the first dependency-ready step. Its walkthrough separates file creation, one named symbol, a command, and cumulative validation. Review assesses captured inputs; a passing result enables one explicit Next. Final completion requires a full-spec assessment. Solutions remain hidden until a separate Reveal request.

This document records verification procedures and limits. It does not declare general model quality, native usability, or release readiness. Evidence classes are defined in [verification.md](verification.md).

## Deterministic and simulated journeys

Run the account-free suite:

```sh
~/.cargo/bin/cargo test -p tandem-companion --test guided_journey
```

The Rust and Python journeys use the production coordinator, SQLite Store, immutable snapshots, research broker, response parsers and evidence gates. Protocol and HTTP I/O are fake. Check receipts are explicitly simulated; these tests cannot establish OS isolation or real model quality.

They cover conversation before drafting; preset and custom question answers; restart before a plan, with a pending question and between passing review/Next; captured local documentation/dependencies; screened public search and retrieved citations; independently edited/saved/approved documents and Start; missing file, named symbol, command and full-spec actions; wrong/missing/failed/stale evidence; exactly-once Next; cancelled/interrupted work; a model change with preserved history; hidden solutions; and unchanged live fixtures. Other focused suites retain broader timing/error permutations.

The same test target checks bounded HTML anchor extraction and strict output schemas recursively for every production action, the subscription opt-in refusal, single-process ledger ownership, durable aggregate caps and narrow diagnosed continuations without resetting caps. Optional fields remain nullable. Host parsing and workflow/evidence authorization remain independent of the wire schema. The wire normalization follows [OpenAI's structured-output schema requirements](https://developers.openai.com/api/docs/guides/structured-outputs#supported-schemas).

## Bounded real checks

These commands require the approved plan scope and the existing managed Tandem ChatGPT account. They refuse before connection without the flag:

```sh
~/.cargo/bin/cargo run -p tandem-companion --example research_probe -- --allow-subscription-usage
~/.cargo/bin/cargo run -p tandem-companion --example guided_journeys -- --allow-subscription-usage
```

The durable ledger is `.tandem-local/guided-companion/approved-suite/turn-budget.json`. Its allowance is shared across private turns and separate public workers: probe 4, Rust 24, Python 24, aggregate 52. A reservation is synchronized to disk before an operation can dispatch its single turn; failed preflight reservations are never refunded. The ledger's exclusive lock and single-attempt claims prevent concurrent or fresh-budget retries. A damaged ledger or stale lock stops dispatch for inspection. Do not delete or replace it to rerun a failed example.

The real suite uses original disposable fixtures and independently authored boundary/caller oracles. It logs programmatic human choices separately: domain answers, document edits, exact approvals, Start, installed check selection and alternative code entered as unsaved overlays. These are harness choices, not usability observations. Generated plans use the production one-to-32-step bounds; the harness accepts their legitimate count and never forces a single step. Multiple tiny file/symbol/command actions and every required review remain mandatory. It requires actual runtime review to reject wrong/missing evidence and accept a valid alternative before advancing. No Reveal is requested. Live fixture hashes and unchanged original tests/callers are checked throughout.

There is no API billing, provider or model fallback. Automatic uses the runtime's unique advertised account default and supported default effort. Missing auth, quota, model, capability or network stops the affected check. Search events are real runtime observations; a citation requires a separate bounded anonymous HTTPS receipt. Query-bearing, PDF, oversized and otherwise unsupported sources remain explicit limitations.

### Observed real results on 2026-11-15

| Check | Observation | Class / result |
| --- | --- | --- |
| Prerequisites | Pinned Codex 0.169.0, runtime SHA-256 `739cda86e15d30a6e2b40cf8dcbbfb988aaf48b305b27065573bb8e9fd41d002`; managed ChatGPT included usage available; Automatic resolved `gpt-6.1-sol`, effort `low` | R passed; zero model turns |
| Hosted search | Actual started/completed `webSearch` events for public Rust integer documentation | R passed; one probe turn |
| Source retrieval | Three search-returned `u32` pages returned HTTP 200 but exceeded 1 MiB. A directly requested smaller official [integer minimum page](https://doc.rust-lang.org/std/cmp/fn.min.html) returned HTTP 200, 7,033 downloaded bytes, body SHA-256 `8d0450a49714d8811120473ffa37f4e63d94c653ab0d42bf908086b1785ab665` | R passed after source selection; no new search/model turn; original limitations retained |
| First private discussion | Both languages received HTTP 400 `invalid_json_schema`: `Invalid schema for response_format 'codex_output_schema': 'schema.required' is required to include every key in properties. Missing 'question'.` | R failed; diagnosed and wire schema fixed with a failing deterministic regression |
| Same-suite schema continuation | Both languages then returned structured discussion responses, but combined a clarification question with research requests. The coordinator rejected each with `Resolve research before asking a clarification` | R failed closed; deterministic host staging subsequently fixed and regression tested across all five conversational actions |
| Same-suite research/question continuation | Both languages completed eight private research rounds while official public URLs were repeatedly withheld. No structured clarification was published before the operation budget ended | R failed; generic hostname words `lang`/`docs` were falsely classified as private vocabulary; tiny synonym fix has a failing/passing privacy regression; no implementation progress established |
| Same-suite public-vocabulary continuation | Rust returned 32 research requests and was quarantined by the host's four-request limit; the provider schema now carries that same bound. Python fetched public content but returned no structured question | R stopped; no document/action completion established; exact Rust count/hash diagnostic retained |
| Actual Python section GET | The original `functions.html#min` request had lost its anchor and returned 65,536 bytes/Partial from a 330,541-byte body. Bounded anchor extraction now returns a complete 1,080-byte `#min` receipt with the full body hash | R passed with zero model turns; existing limits preserved; missing/hidden/duplicate anchors are unavailable |
| Final bounded continuation | Rust completed a genuine structured answer and actual local Git/dependency/doc plus public-source receipts, saved/edited/approved a real spec and generated a legitimate four-step plan. The fixture’s arbitrary two-to-three-step rejection was corrected to production bounds, then that unchanged plan was saved/approved and Start recorded with zero further model turns. Python reached its 24-turn cap during DraftSpec without an actual proposal/question | R partial; Rust next PrepareWalkthrough is blocked at 24 turns. Python remains before saved documents at 24 turns. No action or review success claimed |
| Inspection OS profile | Actual `command/exec` protected live/denied sentinels, captured source and tool/environment roots; direct/subprocess/symlink/hard-link attempts were denied; declared artifacts were writable; loopback connection was denied | R passed; 12 assertions, zero model turns |
| Inspection cancellation | Actual child and grandchild PIDs were observed; cancellation returned exit 137 and neither process survived | R passed; zero model turns |
| Isolated validation | Existing real check integrations passed unsaved Rust/Python alternatives and faulty code, hard-link/source/cache protection, network denial, Rust cancellation and missing-dependency classification | R passed; zero model turns; not a completed guided journey |
| Native UI | Seed created without AI. `CGSSessionScreenIsLocked=1`; no GUI launch or automation attempted | M blocked, not passed |

Current charged and confirmed real turn counts: probe **1**, Rust **24**, Python **24**, total **49**. The original failed runs and error-only official `thread/read` diagnostics are preserved. All targeted continuations reused the same session, fixtures, transcript and ledger; it did not replay the human message or create a new suite. The original fixture hashes for that pre-implementation continuation were reconstructed after confirming the tracked fixture matched its initial Git commit; current harness versions persist hashes before the first operation.

Private receipts remain under `.tandem-local/guided-companion/approved-suite/`: `probe-receipt.json`, `probe-search-events.json`, language `receipt.json`, original schema-failure receipts, dispatch schema/source hashes, `sandbox-proof/inspection-result.json`, `sandbox-proof/cancellation-result.json`, and `validation-kernel-receipts.json`. The inspection receipt preserves xcrun cache/FSEvents diagnostic noise; only its separately parsed assertion object establishes the 12 passes. No absence of model shell activity is presented as kernel isolation proof.

Actual Python thread/read diagnostics verify top-level DraftSpec routing and complete selected-source texts/IDs. The final three authorized turns also received the current independent contract in each turn after a bounded draft-context replay fix; earlier payloads relied on its presence in an older thread turn. They still produced no spec. A final routing audit found an overly broad instruction to keep artifacts null until a human answered, despite no pending question. That sentence is now conditional on an actual asked/pending clarification; an explicit draft may propose without first asking a question. Four-intent deterministic wire tests cover no question, pending clarification with ordinary discussion, and explicit answer continuation while preserving every approval/Start gate. No real turn tested that final wording after the original caps were exhausted, and no cause is inferred for every prior model response.

The language runs did not reach implementation actions or AI action review. Rust’s approved documents/Start are retained in `rust/approved-documents.json`; Python’s final cap outcome is in `python/final-draft-spec-continuation.json`. The final Rust responses used the corrected four-request schema. Both per-language caps are exhausted; the three unused probe turns do not extend either language allowance. Their full real journeys and model review of valid alternatives remain **unverified**. No blind retries, extra search probes, unbudgeted quality loops or fallback providers were used. A [sanitized machine receipt](evidence/guided-companion/verification.json) separates successful checks from these remaining requirements.

## Disposable native seed and manual checklist

```sh
~/.cargo/bin/cargo run -p tandem --example guided_ui_seed
```

The command prints exact disposable `project`, `state_dir` and receipt paths under the OS temporary directory, outside ignored repository ancestors. It creates Rust/Python sessions with explicit fixture document commands, approved current actions, independently unapproved documents, preset/custom clarification choices, paged history, a composer draft, deterministic saved response/error history and an unavailable explicit model. It never connects to AI. Do not replace these paths with an installed user profile.

Check the current login-session lock with a bounded ApplicationServices read before GUI work. If locked or unknown, record blocked and stop native work. A frozen screenshot from a locked session does not prove the displayed UI responds. Once unlocked, launch a properly signed disposable QA app with the exact emitted project/profile, preserve existing installed apps and profiles, and record the concrete bundle/code/profile/window identities. Do not toggle macOS global permissions or unlock on the user's behalf.

All rows below currently remain M blocked. Record Rust and Python separately, with actual results, errors and screenshots from the owned current window. Agent observations and user feedback are distinct evidence.

| Scenario | Decisive observation for each language |
| --- | --- |
| Discussion first | Several messages and a follow-up survive restart before any document exists; ordinary chat grants no implementation authority |
| Clarification | Keyboard/pointer can choose a stable preset or custom text, discuss while pending, and resume typed work after restart |
| Research | Local documentation, Git and dependency receipts show their capture/provenance; safe public activity and real citations are visible; excluded/private content and unsupported sources produce honest limitations |
| Document editing | Spec and Plan are distinct views. No-op saves preserve approvals; changed spec reopens both, changed plan reopens only its own approval. Old plan/spec binding is visible |
| Start and gates | Exact approvals enable explicit Start. Menus, keyboard shortcuts, Hint, Reveal and Takeover obey the same current gates; inspection navigation cannot complete or skip an action |
| Tiny actions | Create file, implement one named symbol, command and cumulative validation are separately understandable; each Next advances one action and passing Review does not advance automatically |
| Review evidence | Missing output, wrong argv/cwd, failed/stale/incomplete checks and changed buffers cannot enable current Next. Different valid implementation is accepted against actual independent checks |
| Review/Next restart | Passing review survives restart only for unchanged inputs; duplicate Next does not skip. Final completion requires all spec criteria |
| Hidden solution | Guidance, transcript, streams, notifications and accessibility tree contain no solution. Explicit action/language Reveal remains separate and changes no source or edit grant |
| Model selection | Automatic/default and available explicit choices show exact identities; unavailable explicit selection remains selected and blocks AI; picker is disabled during every active AI phase |
| Errors | Auth, quota, unsupported search/source, malformed reply, disconnect and failed checks preserve input, buffers, documents and honest terminal state without billing fallback or automatic replay |
| Cancel | Independently inspect context capture, account/catalog, local inspection, public search/retrieval, discussion, both drafts, walkthrough, review, Reveal and separately authorized Takeover cancellation; completed effects remain visible and late completions cannot release newer busy state |
| Layout/input | Narrow and wide layouts retain reachable controls and readable metadata; Tab/Shift-Tab, shortcuts, selection, multiline custom text and composer focus work |
| VoiceOver | Core actions have useful names/states; hidden solutions are absent from unrevealed accessible content; no focus theft or essential inaccessible control |
| Offline | With AI unavailable, open/edit/save/undo/search/terminal and separately authorized local checks work; dirty recovery and saved conversation/documents survive restart |

Native AI checks must stay within an explicit remaining usage authorization; the offline seed alone establishes no real model behavior. The old `journeys` example is historical M3 coverage and does not verify this lifecycle or enforce this suite's budget.
