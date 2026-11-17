# Tandem

Tandem is a code editor written in Rust with a Claude chat beside the code. The chat walks you through a feature from discussion to plan to build. You can write the code yourself, with Claude's guidance at the help level you choose, or hand one step, or the rest of the plan, to Claude and then keep or undo each file it changed.

Claude runs through Claude Code on your own Claude subscription. Tandem never asks for or uses an API key.

## Requirements

- macOS on Apple Silicon.
- Node.js 20 or newer. Tandem looks for it on `PATH`, in nvm's installs, and in Homebrew's `/opt/homebrew/bin` and `/usr/local/bin`.
- Claude Code signed in with a Claude subscription: run `claude auth login`. If you do not have the `claude` command, Tandem shows the same command for the copy of Claude Code it ships with, in the chat panel and in Settings (⌘,).
- For development: Rust 1.100.0, which `rust-toolchain.toml` selects. If `cargo` is not on your `PATH`, use `~/.cargo/bin/cargo` as below.

## Run in development

Install the Claude helper's packages once (and again whenever `crates/tandem-claude/helper/package-lock.json` changes), then run the editor on a project folder from the repository root:

```sh
(cd crates/tandem-claude/helper && npm ci)
~/.cargo/bin/cargo run -p tandem -- /path/to/project
```

`~/.cargo/bin/cargo run -p tandem -- --runtime-status` prints which helper and which Node.js Tandem would use, without opening a window.

Tandem keeps its state (open tabs, drafts, chat tasks) in `~/Library/Application Support/Tandem/workspace`, the same folder an installed release uses. This build upgrades that database to schema 6, which older releases cannot open. Add `--state-dir /some/folder` to keep a separate development profile.

## How a task works

1. **Discuss.** Describe the feature in the chat. Claude reads your code, searches the web when it needs to, and asks questions. It does not change files, unless Permissions is set to Bypass and you ask it to.
2. **Plan.** Claude proposes a plan card: the goal, the "done when" criteria, and small numbered steps, each one file to create, one edit, or one command. Ask for changes until it is right, then choose **I'll write it** or **You do it**.
3. **Build.** The active step is pinned above the composer with guidance: what to do, why, where, and code or pseudocode you can copy. Point at a part of the code to see what it does. Write it, then press **Check my work** (⌥⌘C). A pass moves on to the next step.
4. **Hand over when you want.** **Do it for me** lets Claude do the active step; **Build the rest for me** lets it do every remaining step.
5. **Done.** After the last step, Claude checks the task against its "done when" criteria. When they are met, the task is complete.

## Editor commands

The editor uses VS Code's macOS shortcuts: ⌘/ comments a line, ⌥↑/⌥↓ move it, ⌘D adds the next occurrence to the selection, ⌃G goes to a line, ⌘B and ⌥⌘B hide the file explorer and the chat. The search field in the title bar (or ⌘P) finds a file by name, and typing `>` there (or ⇧⌘P) lists every command with its shortcut. The full table is in [docs/tandem/local-use.md](docs/tandem/local-use.md).

## Language tools

Completion, hover, go to definition, references, symbols, formatting and the Problems list come from language servers already installed on your Mac. Tandem never installs them: Settings (⌘,) → **Language tools** shows what it found and, for anything missing, the exact command to install it. Each language's tools stay off until you trust the project and enable them, because they run with your user permissions. Their caches live in Tandem's state folder, not in the project.

| Files | Server | Install |
| --- | --- | --- |
| Rust | rust-analyzer from the project's toolchain | `rustup component add rust-analyzer` |
| Python | Pyright | `npm install -g pyright` |
| Java | jdtls on Java 21 or newer | `brew install jdtls`, `brew install openjdk@21` |
| Swift | SourceKit-LSP from Xcode or the Command Line Tools; Xcode projects also need xcode-build-server | `xcode-select --install`, `brew install xcode-build-server` |
| JavaScript, TypeScript, JSX, TSX | the project's TypeScript 7 (tsgo), or typescript-language-server with the project's TypeScript 6 or older; else the global TypeScript | `npm install -g typescript`, `npm install -g typescript-language-server` |
| Vue | @vue/language-server with typescript-language-server and the project's TypeScript 6 or older | `npm install -g @vue/language-server`, `npm install -g typescript-language-server` |
| Svelte | svelte-language-server | `npm install -g svelte-language-server` |
| HTML, CSS, SCSS, Less, JSON | vscode-langservers-extracted | `npm install -g vscode-langservers-extracted` |

Vue files are highlighted with the HTML grammar. Java imports Maven and Gradle projects offline, so build once from the command line first. In an Xcode project, run the `xcode-build-server config` command Settings shows.

## Look and fonts

Settings (⌘,) → **Appearance** has a window opacity slider (30–100%; it fades the whole window, text included, so you can see what is behind it) and a blur strength slider for what shows through (0 turns the blur off). Settings → **Fonts** sets the font and size of the editor, the terminal, the chat and the file explorer, with ligatures for the code fonts. The defaults are FiraCode Nerd Font for code and the system font for text; a font that is not installed is replaced by Menlo or the system font. Everything applies to all projects.

## Help levels

Each language has its own help level, Beginner by default. Change it on the step card or in Settings.

- **Beginner:** the complete code for the step, with every keyword, construct and function call explained when you point at it; **Open file** takes you to the line of the change.
- **Intermediate:** a description, where the change goes, and pseudocode; you write the code. Only what is new or unusual in the language is explained.
- **Expert:** a short description and where the change goes.

Every level explains why. Claude explains each language on its own terms, without comparing it to others.

## Takeover, Keep and Undo

Before Claude takes over, Tandem saves your unsaved files and takes a snapshot of the project. While Claude works, the editor is read-only and ⌘. stops it. Afterwards a card lists every file Claude created, modified or deleted, including files changed by commands it ran. Press **Keep** to accept a change, or **Undo** to put the file back as it was. Undoing everything returns the plan to the step where the takeover started.

## Permissions

The **Permissions** button next to Model and Effort decides how often Claude asks before it acts. The choice is saved for all projects.

- **Ask** (the default): an Allow/Deny card for each command Tandem does not allow by itself. Claude edits files only during a takeover.
- **Auto**: Claude Code's auto mode. Its safety check lets routine commands run and asks only about risky ones. Claude still edits files only during a takeover.
- **Bypass**: nothing asks. Every command runs, and Claude may edit project files in any turn when you ask it to. The button is shown in the warning colour.

In every mode Claude's edit tools are refused outside the project folder. Keep and Undo exist only for a takeover: a command that runs in another turn, and in Bypass an edit outside a takeover, has no snapshot and cannot be undone in Tandem. The details are in [docs/tandem/companion.md](docs/tandem/companion.md#what-claude-may-do).

## Tests

```sh
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --check
(cd crates/tandem-claude/helper && npm ci && npm test)
python3 -m unittest discover -s scripts/tests
```

No test starts a real Claude turn: the Rust tests drive a scripted fake helper, and the helper's tests use a mock of the Claude Agent SDK.

## Packaging

Building, verifying, installing and rolling back a local release are described in [docs/tandem/local-use.md](docs/tandem/local-use.md).

## Limits

- UTF-8 text files up to 2 MiB, and at most 32 open tabs.
- One task and one Claude turn at a time.
- The plan is fixed once Build starts; a different plan needs a new task.
- Undo does not cover ignored files (for example anything in `target/` or `node_modules/`), and cannot restore files over 2 MiB.
- Keep and Undo cover takeovers only. Commands Claude runs in other turns, and its edits outside a takeover in Bypass, cannot be undone in Tandem.
- A packaged Tandem is a local ad hoc build for your own Mac: it is not notarized, and it contains Anthropic's proprietary Claude Code binary, so it must not be redistributed.

## More

- [docs/tandem/companion.md](docs/tandem/companion.md): the design in detail: the flow, what Claude may do, subscription and privacy rules, the architecture.
- [docs/tandem/local-use.md](docs/tandem/local-use.md): installing, first run, shortcuts, recovery and building a release.
- [docs/tandem/third-party-notices.md](docs/tandem/third-party-notices.md): what the third-party notices of a release cover.
- Historical documents from the earlier Codex-based design, kept for reference: [the original plan](docs/tandem/README.md), [product](docs/tandem/product.md), [architecture](docs/tandem/architecture.md), [UX](docs/tandem/ux.md), [roadmap](docs/tandem/roadmap.md), [verification](docs/tandem/verification.md), [guided companion verification](docs/tandem/guided-companion.md), [native glass refresh](docs/tandem/native-glass-refresh.md), and the milestone reports [M0](docs/tandem/m0-progress.md), [M1](docs/tandem/m1-progress.md), [M2](docs/tandem/m2-progress.md), [M3](docs/tandem/m3-progress.md), [M4](docs/tandem/m4-progress.md) and [M5](docs/tandem/m5-progress.md).
