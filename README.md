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

1. **Discuss.** Describe the feature in the chat. Claude reads your code, searches the web when it needs to, and asks questions. It does not change files.
2. **Plan.** Claude proposes a plan card: the goal, the "done when" criteria, and small numbered steps, each one file to create, one edit, or one command. Ask for changes until it is right, then choose **I'll write it** or **You do it**.
3. **Build.** The active step is pinned above the composer with guidance: what to do, why, and code or pseudocode you can copy. Write it, then press **Check my work** (⌥⌘C). A pass moves on to the next step.
4. **Hand over when you want.** **Do it for me** lets Claude do the active step; **Build the rest for me** lets it do every remaining step.
5. **Done.** After the last step, Claude checks the task against its "done when" criteria. When they are met, the task is complete.

## Help levels

Each language has its own help level, Beginner by default. Change it on the step card or in Settings.

- **Beginner:** the complete code for the step, with every keyword, construct and function call explained.
- **Intermediate:** a description and pseudocode; you write the code. Only what is new or unusual in the language is explained.
- **Expert:** a short description.

Every level explains why.

## Takeover, Keep and Undo

Before Claude takes over, Tandem saves your unsaved files and takes a snapshot of the project. While Claude works, the editor is read-only and ⌘. stops it. Afterwards a card lists every file Claude created, modified or deleted, including files changed by commands it ran. Press **Keep** to accept a change, or **Undo** to put the file back as it was. Undoing everything returns the plan to the step where the takeover started.

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
- A packaged Tandem is a local ad hoc build for your own Mac: it is not notarized, and it contains Anthropic's proprietary Claude Code binary, so it must not be redistributed.

## More

- [docs/tandem/companion.md](docs/tandem/companion.md): the design in detail: the flow, what Claude may do, subscription and privacy rules, the architecture.
- [docs/tandem/local-use.md](docs/tandem/local-use.md): installing, first run, shortcuts, recovery and building a release.
- [docs/tandem/third-party-notices.md](docs/tandem/third-party-notices.md): what the third-party notices of a release cover.
- Historical documents from the earlier Codex-based design, kept for reference: [the original plan](docs/tandem/README.md), [product](docs/tandem/product.md), [architecture](docs/tandem/architecture.md), [UX](docs/tandem/ux.md), [roadmap](docs/tandem/roadmap.md), [verification](docs/tandem/verification.md), [guided companion verification](docs/tandem/guided-companion.md), [native glass refresh](docs/tandem/native-glass-refresh.md), and the milestone reports [M0](docs/tandem/m0-progress.md), [M1](docs/tandem/m1-progress.md), [M2](docs/tandem/m2-progress.md), [M3](docs/tandem/m3-progress.md), [M4](docs/tandem/m4-progress.md) and [M5](docs/tandem/m5-progress.md).
