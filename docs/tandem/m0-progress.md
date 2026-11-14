# M0 feasibility work

The user approved the initial plan on 2026-11-13. Implementation is authorized within that plan; material architecture or subscription changes still require review.

## M0-A working plan

Build a small Rust stdio client against the installed Codex 0.169.0. Generate schemas from that exact executable, verify the credential-store namespace, and use a private Tandem Codex home with ChatGPT-only authentication. Run one short structured model turn, a dependent follow-up, a child restart/thread resume, and bounded cancellation. Do not read/copy existing credentials or change global settings.

Validation: framing/error tests with simulated messages, then the actual subscription integration. Receipts contain selected metadata and harmless fixture answers, never raw authentication responses or process logs. An unavailable login or quota is not a successful integration gate.

Status: M0's focused feasibility checks passed on the recorded Mac/runtime. Native-editor and execution results and their limits are below. The main application (M1 onward) has not started; the evidence supports proceeding with the approved architecture.

## Independent work while managed sign-in is pending

M0-B: pin GPUI Kit 0.9.1 and build the smallest two-buffer native editor window. Verify the dependency/compiler baseline before adding product behavior. No framework substitution is authorized by this probe.

M0-C: use a separate signed-out Codex home and disposable sentinel files to test explicit permission profiles through `command/exec`. First prove allowed input reads, denied outside reads/writes and artifact-only writes without model access. These execution tests do not substitute for the subscription gate.

## Results recorded on 2026-11-13

### M0-A: passed

- The Rust client connected to the installed Codex 0.169.0 over stdio and used managed ChatGPT browser sign-in. The user completed sign-in in Tandem's separate Codex home.
- Real model answers were **42 → 51 → 102**: initial calculation, dependent follow-up, then a follow-up after shutting down/restarting app-server and resuming the same thread. The available default model was `gpt-6.1-sol`.
- Interruption reached a confirmed `interrupted` terminal state. The client waits for `turn/started` before interrupting; `turn/start` can return before the runtime considers the turn active.
- Successful fixture results were recovered from persisted thread history after a later probe error, without repeating those completed model calls. Future runs checkpoint receipts after each successful phase.
- Streamed deltas were also observed in the subsequent real-model boundary test. That test now requires actual execution denial markers; unchanged files alone are insufficient evidence.
- The global Codex configuration fingerprint remained unchanged, the existing CLI still reported ChatGPT authentication, and Tandem had no plaintext `auth.json`. Codex manages its private Keychain entry; Tandem does not parse/copy tokens.

Evidence: [subscription receipt](evidence/m0/subscription-receipt.json), [runtime/schema fingerprints](evidence/m0/runtime-contract.json). Stable schema: 329 JSON files; experimental schema: 479. These are specific to the inspected executable.

### M0-C: core execution boundary demonstrated

Fifteen real local checks passed, including allowed input reads, denied outside reads, parent/symlink escapes, subprocess reads, companion source writes, artifact-only validation writes, per-file takeover writes, denied ungranted writes and a denied connection to a reachable loopback endpoint. A real Cargo build script/test generated an allowed artifact while its protected-source/outside read/write attempts were denied. A streaming command was terminated and its spawned child was confirmed gone.

These command checks ran through a **signed-out** Codex profile. This establishes an offline local validation path for the tested runtime; it does not depend on an available model turn.

A separate ChatGPT-backed model ran a disposable boundary script. Its event output contained all three required markers: outside read denied, source write denied and outside write denied. Protected contents were unchanged and denied sentinel contents never appeared in the event stream. The runtime reported an empty automatic instruction-source list despite the hostile `AGENTS.md` fixture. A project-local configuration fixture did not grant broader access.

Evidence: [local command checks](evidence/m0/boundaries-receipt.json), [model execution checks](evidence/m0/model-boundary-receipt.json).

Limits: this is a focused macOS/runtime feasibility result, not a proof covering every future tool, plugin, runtime release or platform. The application-owned grant/state/import layer remains M2/M4 work. Full crash recovery, persisted operation reconciliation and broader tool-configuration inventories still need implementation coverage.

### M0-B: native foundation demonstrated

The native probe builds and launches with GPUI Kit 0.9.1 and its exact GPUI 0.3.12 snapshot. Rust 1.95 could not compile the upstream macOS backend's newer library usage. Tandem now pins stable Rust 1.100.0 locally; the user's global default remains 1.95.0.

Observed checks:

- Real native UI: two editor buffers, syntax highlighting, readable code/task/status labels and a PTY terminal pane.
- Unicode paste, grouped undo/redo, find through the keyboard, and macOS Option-E/E dead-key composition.
- Real rust-analyzer completion filtering/insertion, definition mapping and unsaved diagnostics. In the UI, an incomplete function produced two diagnostics; undo and a fresh check cleared them. Later edits invalidate displayed diagnostic state; late results are discarded if their snapshot no longer matches.
- Terminal engine/PTY: Unicode and ANSI parsing, resize, Ctrl-C, usable shell input/output and child cleanup. The current probe renderer is monochrome; full terminal selection, scrollback UI and IME are not implemented.
- On the final probe build, a 20,000-line, 1,068,890-byte buffer: 40 insertions measured p95 **3.33 ms**, maximum **5.41 ms**, from programmatic editor mutation to the native frame callback in a debug build on Apple M5 Pro/48 GiB. Unicode/combining characters and CRLF survived the snapshot/file round trip.
- The user explicitly confirmed that typing and editing feel normal in the native probe.
- With VoiceOver enabled, the user explicitly confirmed that it reads code and navigates to the diagnostic action correctly. This is user-reported screen-reader validation, separate from the agent's accessibility-tree inspection. VoiceOver was confirmed off again afterward, matching its original setting; no extra toggle was sent once off was observed.

The timing is a limited internal measurement, not physical keyboard-to-photon latency. It excludes compositor presentation, real IME sessions and concurrent model streaming. It supports feasibility but does not establish the entire performance target across real workloads.

Evidence: [language checks](evidence/m0/language-receipt.json), [terminal checks](evidence/m0/terminal-receipt.json), [editor measurement](evidence/m0/editor-measurement.json). Broader IME, long coding sessions and an exhaustive accessibility audit are not yet claimed as passed. The basic VoiceOver check above is complete.

## Code validation

Nine focused deterministic/simulated tests passed: JSONL framing/limits/redaction, explicit child environment, unknown server-request rejection with interleaved events, deferred response routing, disconnect/timeout behavior, LSP framing and Unicode position conversion. Real integration checks above are separate from those nine tests.

Builds, formatting and strict Clippy checks passed. Selected receipts were checked for valid JSON, working local links and accidental authentication material. No commits or publishing have been performed.

## What exists and what does not

The repository has a small Rust workspace containing the Codex transport/probe, a bounded single-document LSP adapter and a native editor/terminal probe. The open application uses disposable buffers; it does not yet open/save user projects, implement task guidance/review state, or provide takeover/import. It is not the first usable IDE release.

Next: implement M1's real project/buffer/save/recovery foundation under a brief milestone plan. The approved architecture and subscription requirement remain unchanged. No new plan approval is needed for that agreed work.
