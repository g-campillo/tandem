# Tandem

A standalone Rust IDE built around writing code with an AI companion. **M5 provides the first locally installable macOS release**, including subscription-backed guidance/review, Rust/Python tools, scoped takeover, safe draft import/undo and measured improvement proposals. See the [local usage guide](docs/tandem/local-use.md) and [M5 verification](docs/tandem/m5-progress.md).

Start with the [approved plan](docs/tandem/README.md), [M4 results](docs/tandem/m4-progress.md), [M3 implementation and verification](docs/tandem/m3-progress.md), [M2 foundation](docs/tandem/m2-progress.md), [M1 editor results](docs/tandem/m1-progress.md), and [M0 feasibility evidence](docs/tandem/m0-progress.md).

## Launch the installed app

Open `~/Applications/Tandem.app`. The launcher selects an immutable local release under `~/Applications/Tandem Releases/`; older versions and recovery data remain intact. Codex is bundled and fingerprint-verified. This is an ad hoc signed local Apple Silicon build, not a notarized public distribution.

Use **App** (⌘,) for runtime/tool health, recovery locations and notices. Editing works without AI. [Installation, rollback and build instructions](docs/tandem/local-use.md).

## Run the development editor

```sh
~/.cargo/bin/cargo run -p tandem -- /path/to/project
```

Or build and package a development macOS app:

```sh
~/.cargo/bin/cargo build -p tandem
python3 scripts/package-macos-dev.py
```

The current bundle is `.tandem-local/m5/Tandem.app`, named **Tandem M5** with its own bundle identifier. It is signed ad hoc for local development, not notarized. Normal launches store recovery data in `~/Library/Application Support/Tandem/workspace`. M5 verification uses `.tandem-local/m5/qa-state`; earlier bundles/profiles remain separate. Use `--state-dir` to select a profile. Targeting an older destination requires explicit `--profile m1`, `--profile m2`, `--profile m3` or `--profile m4`.

Use **Open Folder**, **Recent**, or a folder argument. Tabs retain independent undo while open. Drafts are checkpointed locally without automatically saving project files. New nested paths remain drafts until explicit Save creates their validated parent folders. **Changes** compares a buffer with disk; **Use Disk Version** is explicit and undoable within that session. **Save Copy** preserves a draft when the original cannot be saved.

| Action | Shortcut |
| --- | --- |
| Open folder / quick open / new file | ⌘O / ⌘P / ⌘N |
| Save / save all / save copy | ⌘S / ⌥⌘S / ⇧⌘S |
| Find / replace / project search | ⌘F / ⌥⌘F / ⇧⌘F |
| Compare with disk / close file | ⇧⌘D / ⌘W |
| Toggle terminal / text size | Control-` / ⌘= and ⌘− |
| Tools and task / symbols | ⇧⌘L / ⇧⌘O |
| Format buffer / references / definition | ⇧⌥F / ⇧F12 / F12 |
| Ordinary symbol completion | Control-Space |
| Analyze / guide / review | ⌥⌘A / ⌥⌘G / ⌥⌘R |
| Takeover / undo agent import preview | ⌥⌘T / ⌥⌘Z |
| Stop AI work / application health | ⌘. / ⌘, |

The terminal starts only when requested. It uses a basic interactive `/bin/sh`, a 2,000-line scrollback buffer, and the selected project directory. Its PATH is extended inside the child process for installed Cargo/Homebrew tools; global shell settings are not edited.

**Companion & Tools** opens the companion, tool setup, problems, checks and manual-task panels. Enable installed Rust or Python tools after reviewing the project-trust notice. These run with your user permissions. Rust proc macros/build scripts and automatic Cargo checks are disabled. Pyright uses a selected installed interpreter; Python formatting is not configured. **Problems** shows version-associated diagnostics and navigation results. Explicit Rust formatting is undoable.

**Companion** uses your ChatGPT subscription through the official Codex runtime. Connect or use ChatGPT Sign In; describe a feature and analyze its context, then edit/accept the proposed plan. Work on a step, ask questions, request hints/explanations, reveal an optional read-only example, and review your implementation. These companion actions grant no AI editing authority. Guidance preferences resolve per field from global, language, project, project-language and temporary step settings. The current guided language is explicit, including in mixed projects. Solutions start hidden; review is explicit.

**Checks** offers Cargo check/test, Python unittest/pytest and a fixed Python comparison benchmark recipe. Review each execution envelope once; unchanged permission scopes are reused. Validation includes eligible live buffers and retained closed drafts in an isolated input copy. Source inputs and tool environments are read-only; artifact writes are isolated and tool networking is denied. A selected project virtualenv has explicit read-only path exceptions. Cargo stays locked/offline; missing dependencies and empty Python test discovery remain unvalidated. Dependency metadata affects evidence compatibility without broadening the saved permission grant. No model sign-in is needed for local checks.

**Takeover** prepares an isolated working copy for an accepted step or feature. Review exact writable paths, checks and limits before authorizing. The model returns structured edit/check requests; Tandem enforces the scope. Stop returns control with completed actions recorded. Review a staged diff, then preview/import changes into unsaved buffers. Import and **Undo Agent Import** preserve non-overlapping user edits and block conflicts; neither writes source files to disk. Confirming a revealed solution is unrelated to takeover authority.

**Clarity / Speed / Balanced** produce proposals with preconditions, behavioral invariants, tradeoffs and validation gaps. Actual benchmark receipts appear separately from expected complexity. The Python comparison recipe expects `benchmarks/tandem_compare.py` with `cases()` yielding 1–12 `(label, input)` pairs and `baseline(input)` / `candidate(input)` returning comparable values. Its fixed harness checks equality, runs two warmups and seven alternating samples, and excludes input copying/setup from timing. Project callbacks are trusted like tests; this is sampled evidence, not a proof of equivalence. No arbitrary benchmark commands or Rust benchmark integration yet.

Plans, current steps, preferences, conversations, review bindings and check receipts are local operational state. Compiler/test evidence, model assessment and your manual completion decision remain separate. Editing code invalidates applicable review state; interruption/restart never automatically replays an uncertain model turn. Ordinary editing works disconnected, and no API-key billing fallback is provided.

Current limits include UTF-8 files up to 2 MiB, 32 open tabs, bounded context/validation capture, no persisted undo history or cursor/scroll restoration, and monochrome terminal rendering. Unsent question/form edits are not checkpointed. See the M5 report for verification boundaries and known limitations. M6 will exercise developing Tandem inside Tandem.

## Development baseline

- macOS on Apple Silicon; currently exercised on macOS 27.0.1.
- Rust 1.100.0, selected by `rust-toolchain.toml`. This does not change the user's global Rust default.
- GPUI Kit 0.9.1 and its pinned GPUI snapshot.
- Codex CLI 0.169.0 for probes and isolated checks, with a pinned executable fingerprint. The installed app contains the tested Codex/exec-server pair. Development builds use the pinned vendor package; Rust checks still need an installed Apple linker/SDK.
- rust-analyzer and rust-src installed for Rust 1.100.0 for the language fixture.
- Pyright 1.1.414, Node 24.22.1 and Python 3.14.6 for the Python fixtures. Installed pyenv environments and project `.venv` are supported; other installations need their own validation. No automatic dependency/environment installation occurs in the app.

If Cargo is not on PATH, invoke `~/.cargo/bin/cargo` in the commands below.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p tandem-editor-probe
```

The native probe has Rust/Python editor controls, real Rust completion/hover/definition hooks, explicit unsaved-buffer diagnostics and a basic human terminal. It uses disposable files under `~/Library/Caches/Tandem/`. It does not open or save the user's project files, expose an AI task workflow, or represent the first usable release.

## Explicit integration checks

These commands execute real local processes; the subscription commands consume a small amount of the signed-in ChatGPT allowance.

```sh
cargo run -p tandem-probe -- status
cargo run -p tandem-probe -- login
cargo run -p tandem-probe -- turns
cargo run -p tandem-probe -- boundaries
cargo run -p tandem-probe -- model-boundaries
cargo run -p tandem-editor-probe -- --lsp-check
cargo run -p tandem-editor-probe -- --terminal-check
cargo run -p tandem-editor-probe -- --editor-check
cargo run -p tandem-lsp --example integration
cargo run -p tandem-checks --example integration
cargo run -p tandem-lsp --example python
cargo run -p tandem-checks --example python
cargo run -p tandem-checks --example python -- --venv --copies
cargo run -p tandem-companion --example smoke
cargo run -p tandem-companion --example smoke -- --resume
cargo run -p tandem-companion --example journeys
cargo run -p tandem-agent --example integration
cargo run -p tandem-checks --example benchmark
cargo run -p tandem-companion --example improvement
cargo run -p tandem-companion --example improvement -- --keep
```

`login` opens Codex's official managed browser flow. Authentication belongs to a separate `~/Library/Application Support/Tandem/codex-m0` home and Codex's Keychain entry. No credential-copying, API-key fallback or global Codex settings changes are involved. `recover` is a probe-specific recovery operation for the most recent arithmetic fixture; it is not a general conversation recovery UI.

The companion reuses the existing Tandem-owned managed credential home without copying credentials or rewriting its stored configuration; restrictive settings are process overrides. It keeps main conversations and solution previews in separate task/language threads. Codex memory generation/use, model execution/editing tools and plugins are disabled. Disconnect closes Tandem's connection; it does not erase the managed sign-in.

Private/runtime-generated artifacts live in `.tandem-local/` and user-local Tandem state/cache directories. Selected sanitized evidence is retained under `docs/tandem/evidence/`. Do not commit the private Codex home, raw protocol streams or authentication URLs.
