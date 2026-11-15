# Tandem

A standalone Rust IDE built around a persistent AI conversation: research the task, edit and explicitly approve its Spec and Plan, Start implementation, then complete one action at a time with AI Review and explicit Next. Rust/Python tools, isolated checks, scoped Takeover and draft import/undo remain separately authorized. See the [local usage guide](docs/tandem/local-use.md) and [guided companion results and limits](docs/tandem/guided-companion.md); M5 is the historical first installable release.

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

The development packaging script produces `.tandem-local/m5/Tandem.app`, named **Tandem M5** with its own bundle identifier. Versioned guided-companion release bundles are built and verified separately under `.tandem-local/releases/`; building one does not install it. It is signed ad hoc for local development, not notarized. Normal launches store recovery data in `~/Library/Application Support/Tandem/workspace`. M5 verification uses `.tandem-local/m5/qa-state`; earlier bundles/profiles remain separate. Use `--state-dir` to select a profile. Targeting an older destination requires explicit `--profile m1`, `--profile m2`, `--profile m3` or `--profile m4`.

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

**Companion & Tools** opens conversation/document/action views, tool setup, problems and checks. Enable installed Rust or Python tools after reviewing the project-trust notice. These run with your user permissions. Rust proc macros/build scripts and automatic Cargo checks are disabled. Pyright uses a selected installed interpreter; Python formatting is not configured. **Problems** shows version-associated diagnostics and navigation results. Explicit Rust formatting is undoable.

**Companion** uses your ChatGPT subscription through the official Codex runtime. Discuss the task and answer clarification choices or custom text. Read-only local research and screened public-source receipts inform separate editable Spec and Plan proposals. Save and explicitly approve both exact versions, then **Start Implementation**. Changed documents reopen affected approvals; ordinary chat grants no implementation authority. Each tiny action requires a current passing **Review**, followed by one explicit **Next**. Failed, missing or stale evidence blocks advancement; completion includes full-spec validation. History survives restart. Legacy plans continue through a linked session with the missing documents/approvals.

The global model picker uses the live catalog: Automatic resolves the account default at run start; an unavailable explicit selection stays selected and blocks AI. Active operations show local loading/phase state and Cancel, with the picker disabled. Solutions start hidden; a separate current-action Reveal or an explicitly selected AutomaticPreview preference grants no edits. Guidance preferences still resolve per field through global, language, project, project-language and temporary step settings. Research sends only a screened generic brief to its separate public worker, without private code, documents or conversation.

**Checks** offers Cargo check/test, Python unittest/pytest and a fixed Python comparison benchmark recipe. Review each execution envelope once; unchanged permission scopes are reused. Validation includes eligible live buffers and retained closed drafts in an isolated input copy. Source inputs and tool environments are read-only; artifact writes are isolated and tool networking is denied. A selected project virtualenv has explicit read-only path exceptions. Cargo stays locked/offline; missing dependencies and empty Python test discovery remain unvalidated. Dependency metadata affects evidence compatibility without broadening the saved permission grant. No model sign-in is needed for local checks.

**Takeover** requires the current Spec/Plan approvals and Start, then prepares an isolated working copy for a separately authorized step or feature. Review exact writable paths, checks and limits before authorizing. The model returns structured edit/check requests; Tandem enforces the scope. Stop returns control with completed actions recorded. Review a staged diff, then preview/import changes into unsaved buffers. Import and **Undo Agent Import** preserve non-overlapping user edits and block conflicts; neither writes source files to disk. Imported buffers still need the same current-action AI reviews. Document changes invalidate old workflow grants. Revealing a solution grants no Takeover authority.

**Clarity / Speed / Balanced** produce proposals with preconditions, behavioral invariants, tradeoffs and validation gaps. Actual benchmark receipts appear separately from expected complexity. The Python comparison recipe expects `benchmarks/tandem_compare.py` with `cases()` yielding 1–12 `(label, input)` pairs and `baseline(input)` / `candidate(input)` returning comparable values. Its fixed harness checks equality, runs two warmups and seven alternating samples, and excludes input copying/setup from timing. Project callbacks are trusted like tests; this is sampled evidence, not a proof of equivalence. No arbitrary benchmark commands or Rust benchmark integration yet.

Sessions, versioned documents/approvals, action history, preferences, review bindings and check receipts are local operational state in schema 5. Compiler/test receipts, user-attached output and model assessments have distinct provenance. Editing relevant inputs invalidates current review applicability; interruption/restart never automatically replays an uncertain model turn. Older apps refuse schema-5 profiles; rollback preserves data and requires a compatible or separate profile. Ordinary editing and separately authorized local checks work disconnected, and no API-key/provider fallback is provided.

Current limits include UTF-8 files up to 2 MiB, 32 open tabs, bounded context/validation capture, no persisted undo history or cursor/scroll restoration, and monochrome terminal rendering. The composer draft is saved; general unsent question/form edits are not checkpointed. Real Rust/Python fixture journeys passed, while native usability/accessibility remains partially verified; see the guided companion report for exact evidence and unverified scenarios. M6 will exercise developing Tandem inside Tandem.

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

These historical probes execute real local processes; subscription commands consume the signed-in ChatGPT allowance and require their own authorization. They do not replace the current [guarded guided-companion suite and durable caps](docs/tandem/guided-companion.md#bounded-real-checks). Do not rerun the completed real suite as part of release packaging.

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
