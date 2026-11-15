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

The same test target checks bounded HTML anchor extraction and strict output schemas recursively for every production action, the subscription opt-in refusal, single-process ledger ownership, durable aggregate caps, exact authorization amendments and diagnosed continuations without resetting caps. It checks default 24/52 limits, amended 56/116 limits, private/public aggregate charging, reopen, mismatched/missing authorization and preservation of the original reservation prefix. Saved fixture modules have an explicit human-save hash baseline; changes to that baseline, callers or independent oracles fail verification. Optional fields remain nullable. Host parsing and workflow/evidence authorization remain independent of the wire schema. The wire normalization follows [OpenAI's structured-output schema requirements](https://developers.openai.com/api/docs/guides/structured-outputs#supported-schemas).

## Bounded real checks

These commands require the approved plan scope and the existing managed Tandem ChatGPT account. They refuse before connection without the flag:

```sh
~/.cargo/bin/cargo run -p tandem-companion --example research_probe -- --allow-subscription-usage
~/.cargo/bin/cargo run -p tandem-companion --example guided_journeys -- --allow-subscription-usage
```

The durable ledger is `.tandem-local/guided-companion/approved-suite/turn-budget.json`. Its default allowance is shared across private turns and separate public workers: probe 4, Rust 24, Python 24, aggregate 52. A reservation is synchronized to disk before an operation can dispatch its single turn; failed preflight reservations are never refunded. The ledger's exclusive lock and single-attempt claims prevent concurrent or fresh-budget retries. A damaged ledger or stale lock stops dispatch for inspection. Do not delete or replace it to rerun a failed example.

The user's later “Continue on” authorized 32 additional turns per language in the **same** suite: probe 4, Rust 56, Python 56, aggregate 116. The authorization amendment binds the exact repository, plan, suite, ledger, former caps and consumed counts (1/24/24, total 49). An immutable hash of those 49 reservations and the authorization is persisted in the ledger; every extended reopen requires the same explicit authorization file. Missing/corrupt/mismatched authorization fails before connection. The default CLI retains 24/52 caps. No consumed turn, including failed preflight, is refunded.

A targeted continuation uses the original durable session/documents/current action:

```sh
~/.cargo/bin/cargo run -p tandem-companion --example guided_journeys -- \
  --allow-subscription-usage --resume-authorized \
  --authorization /path/to/approved-real-qa-authorization-v2.json --language python
```

Do not repeat a stopped operation without a diagnosed reason. The currently unspent allowance is not permission for blind model-quality loops. The harness emits before/after counts and retains earlier failure receipts. Zero-turn `--diagnose-latest LANGUAGE` and `--dump-latest-schema LANGUAGE` preserve private raw responses locally and expose only contract/schema diagnostics.

The real suite uses original disposable fixtures and independently authored boundary/caller oracles. It logs programmatic human choices separately: domain answers, document edits, exact approvals, Start, installed check selection and alternative code entered as unsaved overlays and explicitly saved owned modules. The current owned-module hash is appended on each human save; research/check operations must match that baseline, while original callers, tests, manifests and other files stay immutable. Actual read-only Git diff/status output may be explicitly attached as UserAttachedOutput/Unvalidated; it is not authenticated local check evidence. These are harness choices, not usability observations. Generated plans use the production one-to-32-step bounds; the harness accepts their legitimate count and never forces a single step. Multiple tiny file/symbol/command actions and every required review remain mandatory. It requires actual runtime review to reject wrong/missing evidence and accept a valid alternative before advancing. No Reveal is requested. Live fixture hashes and unchanged original tests/callers are checked throughout.

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
| Native UI, initial attempt | Seed created without AI; the console was initially locked | M initially blocked; later unlocked observations below |
| Authorized Rust continuation | Preserved the exact approved four-step plan. Created/saved the empty module; rejected wrong behavior; accepted a saved alternative after actual isolated checks; rejected missing command evidence; reviewed every action and cumulative step plus full-spec completion; exactly-once Next and Store restart | R passed at Rust 52; no Reveal or model tools |
| Authorized Python continuation | First resumed DraftSpec and DraftPlan produced saved/edited/approved documents and a legitimate two-step plan; Start was recorded. Inline `-c` command metadata was quarantined. After instruction clarification, two bounded operations repeatedly requested the same public page while asking for local context in prose, without an actual clarification or walkthrough | R partial at Python 43; no implementation/review/Next success claimed |

Current charged real counts are probe **1**, Rust **52**, Python **43**, total **96**. The authorized extension began at **1/24/24 = 49**; Rust ended at **1/52/24 = 77**, and Python ended at **1/52/43 = 96**. Rust has four unused turns, Python thirteen, and the probe three; language caps are independent. No fresh suite, completed probe rerun, refund, unmetered native model turn or provider/model/billing fallback occurred.

Original failures remain in `.tandem-local/guided-companion/approved-suite/` with the source/runtime/schema/model hashes for each dispatch. Rust's saved/approved documents were reused unchanged. Its initial empty-file review correctly requested an actual save; subsequent scope-note limitations and missing preservation evidence were retained, with real Git evidence added explicitly. One zero-dispatch harness deadlock was terminated and recorded with all 71 reservations unchanged; its verified stale lock was removed after the process exited. Earlier module-save baseline metadata was reconstructed only from its exact persisted saved snapshot and explicit human-save log, with that earlier protection gap recorded.

Concrete output-contract fixes retain all independent validators: nonempty bounded `reply.text`; final ValidateStep/ValidateTask coverage; current-action limitation semantics; project-relative cwd; and no inline interpreter scripts. The actual Python post-fix spec success does not prove a cause for all prior empty proposals. Its later repeated PublicPage behavior has no established cause. Zero-turn inspection found all twelve research variants and clarification fields in the strict schema; simulated local request shapes validate, and the pinned provider hands that schema through unchanged. No model responses or missing criteria were fabricated.

Rust `receipt.json` records the complete real journey. Python `receipt.json` and `unfinished-stage.json` record saved approvals/Start and its missing actions, local research and structured answer. Original `probe-receipt.json`, search events, sandbox/cancellation proofs and validation-kernel receipts remain retained; completed probe and OS checks were not rerun. A [sanitized machine receipt](evidence/guided-companion/verification.json) separates D/S, R and M observations and the remaining blockers.

## Disposable native seed and manual checklist

```sh
~/.cargo/bin/cargo run -p tandem --example guided_ui_seed
```

The command prints exact disposable `project`, `state_dir` and receipt paths under the OS temporary directory, outside ignored repository ancestors. It creates Rust/Python sessions with explicit fixture document commands, approved current actions, independently unapproved documents, preset/custom clarification choices, paged history, a composer draft, deterministic saved response/error history and an unavailable explicit model. It never connects to AI. Do not replace these paths with an installed user profile.

Check the current login-session lock with a bounded ApplicationServices read before GUI work. If locked or unknown, record blocked and stop native work. A frozen screenshot from a locked session does not prove the displayed UI responds. Once unlocked, launch a properly signed disposable QA app with the exact emitted project/profile, preserve existing installed apps and profiles, and record the concrete bundle/code/profile/window identities. Do not toggle macOS global permissions or unlock on the user's behalf.

The console later measured unlocked (`IOConsoleLocked=false`, on-console/login true). A uniquely identified, ad-hoc-signed QA bundle used a fresh outside-repository seed and no installed user app/profile. The exact-path CUA lookup ignored its 15-second timeout and was interrupted after 154 seconds; bounded CoreGraphics captures and per-element-timeout AX reads then worked. Fresh AX children must be queried after its initial root-only placeholder refresh. An offscreen AXPress may be a no-op because it is hit tested; verified AX focus followed by unmodified Space activated the actual labeled control.

Observed M evidence: restored discussion and preset/custom question controls; custom multiline input; separate editable Spec, actual version-2 save/exact approval and no-op save preserving that approval; disabled implementation control plus the menu's stale-spec gate; retained unavailable explicit model label; seeded Python current-action metadata, disabled Next and Open in editor; actual Rust/Python typing and Cmd-S verified on disk; narrow/wide metadata wrapping; and exact-app quit/restart retaining the selected Python action and composer draft. The profile still contains only its two original fake model runs: **zero native model turns**. Captures, AX inventories, code/profile/window identities and native actions are under `.tandem-local/guided-companion/native-20261115-resume/`.

This is partial native verification. Spoken VoiceOver, all live AI/cancel phases, busy picker behavior, command attachments, passing-review/Next interaction, full native Rust/Python journeys and complete long-panel scrolling remain unverified. Scroll-driver attempts did not establish scrolling, and are not classified as a demonstrated product defect. The checklist below remains the full target; individual observations above do not imply every row passed. Agent observations and user feedback are distinct evidence.

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
