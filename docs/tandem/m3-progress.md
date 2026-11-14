# M3 — the companion loop with Rust and Python

Status: implemented and verified on the development Mac, including the native checks completed on 2026-11-14. The user authorized continuation after M2. Architecture and subscription requirements are unchanged; takeover remains M4.

## Working plan

1. Add explicit, persisted guidance preferences with per-field provenance: global → language → project → project-language → current-step overrides. Keep solution visibility, explanation depth and instruction detail separate from execution authority. Default to hidden solutions and explicit reviews; no learner tracking.
2. Define bounded task-analysis, guidance, solution and review contracts. Keep proposals separate from accepted plans, and keep suggested solutions in a separate channel generated only after a reveal request or an explicit automatic-preview preference. A revealed solution is display/copy content, never an editor operation. Review findings distinguish demonstrated problems, concerns requiring verification, design tradeoffs and style preferences.
3. Capture immutable project/context snapshots with unsaved overlays, selected source text, input hashes, acceptance/plan revision and check evidence. Show included paths, exclude credentials/ignored/generated inputs, and send changed selected files rather than the whole repository. Use conservative project fingerprints for review applicability. Treat source text and command output as data, not new instructions or permissions.
4. Add a supervised companion client around the pinned Codex 0.169.0 app-server transport: managed ChatGPT authentication, subscription-only checks before turns, thread continuity, bounded streaming, cancellation, sanitized error/usage states and restart recovery. Reuse the existing Tandem-owned managed credential home without reading/copying credentials or changing its stored probe configuration; apply session configuration overrides. Keep all source-writing and executable model tools unavailable. Local validation stays in the separately authorized M2 runner.
5. Extend the language/check adapters to installed Pyright and a selected Python interpreter, with per-project trust and fixed unittest/pytest recipes. Verify unsaved Python diagnostics/completion/navigation and a real isolated test. Node remains a language-tool dependency, not an application frontend. No automatic environment or package installation.
6. Build a task-centered companion panel: connection/context status; analyze/revise/accept plan; current step and dependencies; settings/provenance; hints/explanations/questions; explicit reveal with section explanations; review findings, evidence and stale state; validation and manual completion. Preserve editing while disconnected. Persist accepted state and completed responses; incomplete work resumes as interrupted rather than being replayed.
7. Exercise a complete small Rust and Python journey with real subscription turns and local checks. Add focused deterministic and simulated failure tests for settings, disclosure, authority separation, stale results, cancellation and recovery. Package a separate M3 development app/profile, keeping the M2 bundle and test state intact.

## Acceptance and evidence boundaries

- A real structured companion turn completes using the existing ChatGPT sign-in. Connection/authentication failures never switch billing or discard editing/task state.
- Settings resolve independently per field and guided language; temporary settings expire with their step. Mixed-language steps keep distinct language previews.
- Hidden solutions are not requested or rendered in summaries, accessibility text, logs or previews. Freeform explanatory text is checked before display; model compliance is evaluated separately from deterministic channel/permission enforcement.
- Plan acceptance and revisions are explicit. A valid alternative implementation is accepted against requirements and evidence; a faulty implementation receives a specific, justified next action. Model review and compiler/test results remain separate conclusions.
- Every review records its context, plan and settings versions. Any relevant edit, external source/config change or plan revision makes old findings historical; no automatic completion is inferred from a model answer.
- Revealing/copying suggestions never changes buffers or execution grants. Model-requested tools cannot broaden authority. Explicit compiler/tests may write only the approved artifact directories.
- Editing, task recovery and completed conversations remain usable without AI. Login, quota, transport loss and interruption receive actionable states; no automatic retry can duplicate an uncertain turn.

Observed baseline: Codex CLI 0.169.0, Pyright 1.1.414 (MIT), Node 24.22.1 and Python 3.14.6. The implementation uses the installed generated protocol schemas, local SDK/runtime sources and versioned Pyright documentation. Only behavior identified in the recorded checks below is claimed as validated.

## Implemented result

The desktop now has a companion workspace alongside its existing editor, terminal and local checks. A feature request produces an explicit proposal containing the goal, surrounding architecture, constraints, unknowns, acceptance criteria and dependency-aware steps. Each proposed step has a reason, related files/symbols and completion conditions. The user can edit the goal, criteria, step responsibilities, files and dependencies before accepting a plan. Acceptance creates or revises ordinary task state; it does not modify source files.

The active task and step stay separate from the conversation. **Guide, Hint, Explain, Ask, Reveal Suggested Solution and Review This Step** are deliberate actions. Direct questions and explanations can precede plan acceptance. A requested context expansion adds safe relative paths and retains the original question for the follow-up. The model cannot browse the live filesystem or run a command to expand context itself.

Guidance fields resolve independently through global, language, project, project-language and current-step preferences, with provenance shown in the interface. Step-language overrides are supported and expire on completion/revision. The guided language is explicit rather than being silently changed by the focused editor tab. Hidden/on-request policies never request a solution body; automatic preview is a standing request for an active step/language. Automatic requests ignore unfinished text in the question field and do not replay a completed preview on restart.

Solutions have their own output contract and separate task/language conversation thread. The preview is a read-only editor with optional section explanations and a copy action. There is no insertion/apply action or editing-permission change. Collapsed code is absent from the rendered accessibility tree. Prose channels reject code fences and recognizable complete function bodies before display. This guard is defense in depth, not a formal detector of every possible prose spoiler.

Reviews are bound to selected source hashes, a conservative project fingerprint, accepted plan/step, effective guidance and validation-environment identities. Input edits clear applicability; live-buffer comparisons guard against delayed editor events. Saving identical bytes does not create a different code identity. Old results remain historical and cannot silently become current findings. Compiler/test results, model conclusions and explicit manual completion stay separate. Unsupported claims of a demonstrated problem are downgraded to concerns requiring verification; design/style preferences remain optional. The interface explicitly reports when no matching check evidence was available.

## Runtime, context and persistence

- `tandem-companion` owns bounded context preparation and the supervised Codex exchange. `tandem-core` owns settings, plans, response contracts, applicability and persistence. The desktop coordinates explicit actions and renders approved channels; conversation text is not its state machine.
- The pinned Codex binary speaks its installed experimental protocol over stdio. The app verifies ChatGPT authentication and requires `ordinaryUsageAllowed=true` before a model turn. Unknown included-usage availability fails closed; percentages/reset times cannot establish availability. Reset-credit purchases and automatic reserve fallback are not requested, and API-key billing is never selected.
- Managed sign-in remains Codex-owned, using the existing Tandem-specific credential home and Keychain entry. No credentials are read/copied into application state, no global Codex config is rewritten, and raw protocol/auth URLs are not logged. Restrictive settings are process overrides. Model shell/editing/delegation/plugin tools and memory generation/use are disabled. Unknown server requests are denied; unexpected tool activity ends the operation.
- Main and solution conversations have separate project/task/language bindings. Ordinary completed conversations resume by thread ID, refreshing selected context after process restart. Changed selected files and removals are sent as deltas during a live session. Interrupted/uncertain requests are never replayed automatically; the next explicit request can begin from the saved task and completed decisions.
- Capture is local and bounded before transmission. Eligible live buffers and retained closed drafts override disk. Only selected source text, project configuration, bounded tree metadata and matching evidence are sent. Generated directories, instruction/config directories belonging to agents, symlinks/hard links, binary/oversized inputs and known credential files are excluded. Credential-like text is checked in source, check output and outgoing task/message metadata. These rules can have false positives and are not a universal secret detector; the context inspector shows included and omitted paths.
- SQLite schema 3 adds guidance preferences, model runs and thread bindings, with backup before older-schema migration. A request is journaled before capture/remote work, and drafts are checkpointed before model/check actions. Restart changes unfinished runs to **Interrupted**, retains completed responses and removes uncertain thread bindings. Unsent question and plan-form edits are not yet recovered.
- No model content is shown from streaming fragments. The UI reports operation progress and exposes the validated structured result after completion. Streams/final responses are bounded; no model review is triggered on keystrokes. A guidance change during generation withholds the obsolete response.

## Python and validation

Python discovery uses installed interpreters, a project `.venv` or an explicit interpreter path. Pyright and Node are installed tooling dependencies, not an application frontend. Per-project trust is required before Pyright/interpreter processes start; they have the user's ordinary permissions. Pyright configuration is served through standard LSP workspace settings. Versioned push diagnostics are accepted only for the current document versions; all open Python versions advance when their shared context changes. Unversioned findings are not promoted to current diagnostics. [Versioned Pyright settings](https://github.com/microsoft/pyright/blob/1.1.414/docs/settings.md), [configuration reference](https://github.com/microsoft/pyright/blob/1.1.414/docs/configuration.md).

Fixed unittest and pytest recipes use the selected interpreter in an isolated snapshot. Bytecode/plugin autoloading are restricted, artifact paths are explicit, and tool networking is denied. A project-local virtualenv has narrow read-only exceptions for its runtime paths while original source access remains denied. Both symlink-based and copied virtualenv interpreters were exercised. The app does not install packages or create environments automatically; the verification fixtures create disposable environments without pip.

Permission-grant fingerprints and evidence fingerprints are distinct. Changing dependency metadata invalidates old evidence, but does not expand the stored execution grant or demand another prompt for the same command/resources. An executable or scope change still requires a new grant. Python environment metadata is bounded and includes package records/configuration; this is not a cryptographic audit of every library byte. Tool selector files are bounded and refuse symlink/hard-link reads; malformed selection data is not echoed into error messages.

## Recorded verification

The [verification receipt](evidence/m3/verification.json) records exact current counts, source/package hashes and private evidence paths.

**Automated:** guidance inheritance/reset/expiry; hidden-channel rejection; source/plan/language binding; context deltas; credential exclusions; dependency identity versus permission identity; schema backups; interrupted-run reconciliation and stale-write rejection; retained closed drafts; Unicode/LSP supervision; and the prior editor/terminal/protocol cases. Two test entries are intentionally excluded from ordinary runs: the crash helper is exercised by its parent test, and a real-runtime transport test is invoked separately. Strict Clippy, formatting and the development build pass.

**Real model/backend checks:**

1. A structured analysis completed with existing ChatGPT authentication, then another real turn confirmed conversation continuity after starting a new runtime process.
2. Ten completed fixture turns covered both Rust and Python: analysis, hidden guidance, review of a valid alternative, explicit solution reveal, and review of an incorrect implementation. Both valid alternatives were accepted against actual test evidence; both faulty implementations received justified blocking feedback. No model tool activity or original-source changes occurred in these fixtures.
3. Real Rust and Python language fixtures exercised unsaved cross-file definitions, completion, hover, versioned diagnostics and stale-result rejection. Rust formatting/restart remained passing.
4. Real Cargo/unittest checks used unsaved implementations, passed correct behavior, failed incorrect behavior and preserved originals. Source access, loopback networking and inappropriate hard-link writes were denied; artifacts remained writable. Missing dependencies stayed unvalidated. Python checks also passed in project-local virtualenvs using symlinks and copied interpreters.
5. Stopping an owned Codex transport before a turn produced a failed, preserved request and unavailable connection state with no model turn. Authentication/quota failure classification and sanitization were tested with seeded data; actual quota exhaustion or deliberate sign-out was not forced.

**Observed in the native M3 application:**

- Configured detailed Rust instructions/explanations with a hidden solution and independent high-level Python guidance; effective settings/provenance updated correctly.
- Analyzed a Rust feature, edited its proposed goal, accepted the plan and started the step.
- Requested detailed guidance; the response explained Rust return/range behavior without displaying a complete implementation.
- Typed an alternative implementation into an unsaved buffer, ran an authorized captured Cargo test and received a passing receipt. A model review accepted that implementation using the receipt while the original disk file remained the unfinished fixture.
- Edited the buffer after review and immediately observed the result become historical. Undo did not silently turn a stale finding into a new model verdict.
- Revealed a separate example, attempted typing into its read-only preview, and observed no preview/source change. Collapsing it removed its code and sections from accessibility output.
- Changed assistance during an active request; the response was withheld as **Quarantined**. A separate earlier reveal completed before a preference edit; that race is not counted as in-flight-policy evidence.
- Cancelled a confirmed running subscription turn and observed **Cancelled** with code/history preserved.
- Force-terminated only the verified disposable M3 process during a later running turn. Reopening recovered the accepted task, active step, preferences and unsaved code, and showed **Interrupted** without replay. Original Rust/Python fixture files were unchanged.

These are bounded, agent-driven observations. Earlier M0 user feedback about typing/VoiceOver remains separate; M3 spoken VoiceOver behavior and broad daily usability were not newly established.

## Native verification follow-up and fixes

After the initial recovery check and a bundle update, computer-use temporarily stopped finding a captureable M3 window (`cgWindowNotFound`). The process remained alive, had checkpointed its restored workspace and was waiting normally in the native event loop. On continuation on 2026-11-14, the same window was accessible again. No rendering-architecture change was needed; the original visibility/automation cause remains unconfirmed.

The remaining native checks then passed:

1. Enabled Python tools, entered an unsaved return-type error, and observed Pyright's specific diagnostic. Replacing it with a valid alternative cleared the finding. An authorized Python unittest run passed against that unsaved alternative while disk stayed unchanged.
2. Asked a direct Python question without an accepted plan and received a concise explanation with no implementation example or quiz.
3. Enabled a Rust-only temporary automatic preview while an unfinished question was present. Exactly one preview completed. Its submitted request did not contain the draft text, which remained in the text box. Selecting Python kept Python's hidden-solution setting and removed the Rust preview from rendered accessibility content.
4. Marked the step complete explicitly and observed the temporary override expire back to inherited hidden solutions. The completed task and source draft survived another restart.
5. Stopped only the app's owned idle Codex child to simulate transport loss. A subsequent request failed without a turn ID; its text, prior responses and source buffers remained available. Editing/undo still worked, and reconnecting did not replay the failed request.
6. Reopened the final ad-hoc-signed bundle, restored the saved task through the UI, and left the verified workspace available for inspection.

This pass found and fixed two usability issues. A lone saved task was previously unreachable after starting a new task; the saved-task selector now includes it. A broken transport previously showed the low-level text “encode request”; it now explains that no start was confirmed and directs the user to reconnect/retry. Cached usage information is cleared when transport/authentication state invalidates it, with a targeted real-runtime regression check. Failed/incomplete requests no longer carry a current-assessment label. Historical test receipts are retained rather than rewritten.

The existing 53-test workspace baseline remains passing; the changed companion component, real transport-failure regression, strict workspace Clippy, formatting, build/signature checks and final native smoke were rerun as appropriate. This closes the M3 milestone gate without claiming broad daily-use or VoiceOver validation.

## Practical limits

Other current limits:

- M3 is a companion preview, not the M5 release. Takeover/import/undo and dedicated optimization/benchmark proposals are M4 work.
- Language support is Rust/Python. Pyright does not provide formatting here. Other Python distributions, very large environments/repositories, public signing/notarization and Windows remain separate gates.
- Context is conservative and bounded; unrelated source/environment changes can invalidate a review. Model judgments are not compiler/test proof, and the real quality checks cover small fixtures, not arbitrary engineering tasks.
- AI reviews currently receive captured source and matching command receipts; language diagnostics are displayed separately. Navigation opens project-local definitions/references; external dependency-source browsing remains later work.
- AI reviews are explicit. Optional after-save AI review is not enabled. Source inspection, hints, questions and optional examples remain available without a quiz or inferred learner profile.
- Input/validation capture is not a filesystem-wide atomic snapshot against unrelated concurrent writers. Each captured file has its own identity; source/environment changes are reconciled before further use.
- The latest 40 companion runs load into the interface. Cache/history retention controls and unsent question/form recovery still need daily-use hardening. No unrequested deletion/cleanup of user data was performed.
- Codex 0.169.0 permission profiles remain experimental, and the development build currently expects known installed runtime/tool paths. M5 must finish distribution and compatibility work.

M4 is next. No architecture or subscription change is recommended.
