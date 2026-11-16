# Using Tandem on your Mac

Tandem is a code editor for macOS on Apple Silicon with a Claude chat beside the code. A packaged release contains the editor and the Claude helper, a small Node program that runs Claude Code through the Claude Agent SDK. Claude Code ships inside the release. To use Claude you need Node.js 20 or newer and a Claude subscription; editing works without either. How the chat, the guided build and the takeover work is described in detail in [companion.md](companion.md).

## Install and launch

Building a package and installing it are separate steps. To install a concrete versioned bundle that you have built (see below) or been given for this Mac:

```sh
python3 scripts/macos_release.py verify /absolute/path/to/Tandem.app
python3 scripts/macos_release.py install /absolute/path/to/Tandem.app
open "$HOME/Applications/Tandem.app"
```

`verify` checks every file of the bundle against its release manifest, the fingerprint of the bundled Claude Code binary, that the bundle contains no symlinks, and the ad hoc code signature. `install` copies the bundle to `~/Applications/Tandem Releases/<release-id>/Tandem.app` and atomically points the launcher link `~/Applications/Tandem.app` at it. Earlier releases and your data stay where they are, and a receipt is written to `~/Applications/Tandem Releases/installation.json`. The installer refuses to replace an unrelated app or link, and never overwrites the files of a release that may be running: quit Tandem before you launch another version.

This is a local ad hoc signed build. It is not notarized and is not tested on other Macs. It contains Anthropic's proprietary Claude Code binary, so it is for your own use and must not be redistributed; see [third-party-notices.md](third-party-notices.md).

From a terminal, the app's executable also takes options: `tandem [FOLDER] [--state-dir DIRECTORY] [--no-restore] [--runtime-status]`. `--runtime-status` prints which Claude helper and Node.js Tandem would use, then exits.

## First run

1. **Node.js.** Install Node.js 20 or newer, for example with nvm or Homebrew. Tandem looks on `PATH`, then in nvm's installs (newest first), then at `/opt/homebrew/bin/node` and `/usr/local/bin/node`. An app opened from the Finder or the Dock gets a minimal `PATH`, so a Node.js installed anywhere else is found only when you start Tandem from a shell that has it on `PATH`.
2. **Open a project** with ⌘O, **Recent**, or a folder argument. The chat panel on the right starts Claude (`Starting Claude…`).
3. **Sign in.** If Claude Code is not signed in, the chat shows `Sign in to Claude` with a command. Copy it, run it in Terminal (it is `claude auth login` for the copy of Claude Code inside Tandem), sign in with your Claude subscription, then press **Reconnect**. Tandem never sees your credentials; Claude Code keeps them.
4. **Settings** (⌘,) shows the connection (`Connected · <subscription> · Claude Code <version>`), the Node.js and helper in use, the **Use my Claude Code setup** switch, the help level per language, the Rust and Python tools, and the recovery folder. Press ⌘, again or **Close Settings** to return to the code.

When Claude cannot be used, the chat says why and offers **Reconnect**: `Claude is unavailable` (for example Node.js or the helper is missing), `Claude subscription required` (Claude Code is set up for an API key or another provider; Tandem only works with a Claude subscription), or `Claude stopped`. If your plan's usage limit is reached, the turn ends with `Your Claude plan's usage limit was reached. Try again later.` Your work is kept in every case, and Tandem never switches to API billing.

**Use my Claude Code setup** is on by default: Claude then loads your Claude Code settings, `CLAUDE.md` files, MCP servers, skills and plugins, and the project's `.claude` folder, as running `claude` in the project would. Turn it off for projects you do not trust.

## Using the chat

- **Discuss.** Type in the composer at the bottom of the chat (⌘L focuses it; Enter sends, Shift+Enter adds a line). Claude reads your code and researches; short lines such as `↳ read src/cli.rs` show what it looked at. It does not change files.
- **Plan.** Claude proposes a plan card with the goal, the "done when" criteria and small steps. Ask for changes in the chat, then choose **I'll write it** (you write the code, Claude guides) or **You do it** (Claude builds it).
- **Guided build.** The active step is pinned above the composer. It shows what to do and why, the code (Beginner) or pseudocode (Intermediate), and a breakdown of the parts. **Copy** copies the code or command, **Open file** opens the step's file at the right line, and **Open Terminal** opens the terminal for a command step. Write the step, then press **Check my work** (⌥⌘C). A pass moves to the next step and Claude prepares its guidance; otherwise the verdict card lists what to fix, and why. Change the help level with `Help: <level> ▾` on the card or in Settings.
- **Takeover.** **Do it for me** lets Claude do the active step, and **Build the rest for me** every remaining step. Tandem first saves your unsaved files, because Claude edits the files on disk. While Claude works the editor is read-only and ⌘. stops it. Afterwards a card lists each file Claude created, modified or deleted, with **Keep** and **Undo** (and **Keep all**, **Undo all**). Undo cannot restore ignored files, such as anything in `target/` or `node_modules/`, or files over 2 MiB.
- **Permission cards.** When Claude wants to run a command that Tandem does not allow by itself, the chat shows the command with **Allow** and **Deny**.
- **Tasks.** The task button in the chat header lists the project's tasks and **New task** (also in the **Chat** menu). The **Model** and **Effort** buttons under the composer choose the Claude model and effort for the next message.
- **Stop.** ⌘. or **Stop** in the composer interrupts Claude at any time.

## Editor basics

Open a folder with ⌘O, **Recent**, or a folder argument; Tandem reopens the last project on the next start. Files open in tabs (at most 32), each with its own undo history while it is open. Unsaved changes are checkpointed in the recovery profile without saving the project file, and come back after a restart (`Recovered unsaved draft · Review before saving.`). **Changes** (⇧⌘D) compares a buffer with the file on disk; **Use Disk Version** is explicit and undoable in that session. When a file changed on disk under your edits, your draft is kept for comparison, and **Save Copy** (⇧⌘S) saves it elsewhere. Files must be UTF-8 text up to 2 MiB.

The panel below the editor has two tabs: **Terminal**, a basic interactive `/bin/sh` in the project folder with 2,000 lines of scrollback, started only when you open it, and **Problems**, the language tools' diagnostics and navigation results. Rust (rust-analyzer) and Python (Pyright) tools are enabled per project in Settings → **Language tools**, after a notice that they run with your user permissions; enable them only for projects you trust.

| Action | Shortcut |
| --- | --- |
| Open folder / quick open / new file | ⌘O / ⌘P / ⌘N |
| Save / save all / save copy | ⌘S / ⌥⌘S / ⇧⌘S |
| Find / replace / search the project | ⌘F / ⌥⌘F / ⇧⌘F |
| Compare with disk / close file | ⇧⌘D / ⌘W |
| Terminal / Problems | ⌃` / ⇧⌘M |
| Text size | ⌘= / ⌘− |
| Format buffer / references / document symbols | ⇧⌥F / ⇧F12 / ⇧⌘O |
| Focus the chat / show or hide the chat | ⌘L / ⇧⌘L |
| Check my work | ⌥⌘C |
| Stop Claude | ⌘. |
| Settings | ⌘, |

## Health, recovery and rollback

Settings (⌘,) → **App and recovery** shows the Tandem version, the helper and Node.js in use, the release id, and the recovery profile, with **Open Recovery Folder** and, in a release, **Open Third-Party Notices**. If the bundled helper is missing or damaged, Tandem says `The bundled Claude helper is missing or damaged. Reinstall a verified Tandem release; editing still works.` Repair it by installing a verified release, not by replacing files inside the bundle.

Tandem's state lives in `~/Library/Application Support/Tandem/workspace` unless you pass `--state-dir`. It holds the database `workspace.sqlite` (open tabs, drafts, preferences and the chat tasks) and, under `takeover/`, the snapshots of takeovers whose files still wait for Keep or Undo. A snapshot is removed once each of its files is decided. One Tandem process uses a state folder at a time. Claude Code keeps its own sign-in and session history outside this folder.

The database is at **schema 6**. Opening an older profile first writes a backup next to it (`workspace-schema<N>-backup-<time>.sqlite`), then upgrades it. Tasks from the earlier Codex-based design stay in the database untouched but are not shown. A Tandem that finds a newer schema than it knows refuses to open the profile and does not change it.

To point the launcher back at the previously installed release:

```sh
python3 scripts/macos_release.py rollback
```

Quit Tandem first. Rollback changes only the launcher link, never the database. A release from before the Claude chat (schema 5) cannot open a schema-6 profile; start it with its own profile instead, for example `"$HOME/Applications/Tandem.app/Contents/MacOS/tandem" --state-dir "$HOME/Library/Application Support/Tandem/older-profile"`. Do not overwrite the current database to force a downgrade.

## Build and verify another local release

You need Rust 1.100.0, Node.js 20 or newer with npm, and network access (npm packages and two public license texts are downloaded). Commit the source you want to package and check that `git status --short` prints nothing. Choose a new release id from the date and the short commit id; packaging never overwrites an existing release. Then, from the repository root:

```sh
RELEASE=claude-companion-YYYYMMDD-SHORTHEAD   # for example claude-companion-20261116-1a2b3c4
(cd crates/tandem-claude/helper && npm ci && npm test)
python3 -m unittest discover -s scripts/tests
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --check
~/.cargo/bin/cargo build --release -p tandem
python3 scripts/prepare-notices.py
python3 scripts/collect-notices.py --output ".tandem-local/notices/$RELEASE"
python3 scripts/macos_release.py package --release-id "$RELEASE" --notices ".tandem-local/notices/$RELEASE"
python3 scripts/macos_release.py verify "$PWD/.tandem-local/releases/$RELEASE/Tandem.app"
".tandem-local/releases/$RELEASE/Tandem.app/Contents/MacOS/tandem" --runtime-status
```

- `npm ci` installs exactly the helper packages in `package-lock.json`, including the Claude Agent SDK and its Claude Code binary.
- `prepare-notices.py` downloads the pinned MIT and Apache-2.0 license texts, writes Tandem's cargo metadata, and checks that the installed helper matches the pins in `scripts/release-inputs.json` (Claude Agent SDK version and Claude Code binary fingerprint). Its private working folder is `.tandem-local/m5`, a name kept from an earlier milestone.
- `collect-notices.py` gathers the notices of Tandem's cargo dependencies and of the helper's npm packages, and must end with `0 unresolved notices`. See [third-party-notices.md](third-party-notices.md).
- `package` refuses notices collected from a different `Cargo.lock` or helper lockfile, unresolved notices, a helper that does not match its pins, and an existing release id. It writes `.tandem-local/releases/<release-id>/Tandem.app`, signed ad hoc.
- `--runtime-status` should print `Claude helper bundled · …/Contents/Resources/claude-helper/main.mjs · Node <version> · <path>`.

Keep the bundle, its inventory and your test notes private under `.tandem-local/`. Verification checks the files and the signature; it does not prove that the app works well, and the build is not notarized. Installing is the separate `install` command shown at the top.

### Development bundle

For a quick app bundle of a debug build, with the helper but without notices:

```sh
(cd crates/tandem-claude/helper && npm ci)
~/.cargo/bin/cargo build -p tandem
python3 scripts/package-macos-dev.py
```

It prints the path of `.tandem-local/m5/Tandem.app`, an app named **Tandem M5** after an earlier milestone, with its own bundle identifier. It has no notices folder, so Settings shows no **Open Third-Party Notices** button.

## Check the UI offline

`chat_ui_seed` writes a small Rust project and a state folder with three tasks, so the chat panel can be checked by hand without Claude and without using your subscription:

- `Add a --json flag to the CLI`: a conversation and a plan card in the Plan phase;
- `Add a --json flag to the CLI (guided)`, selected at start: a guided build at step 1 of 4 with stored Beginner guidance;
- `Add a --json flag to the CLI (Claude did step 1)`: a takeover's three changed files waiting for Keep or Undo.

```sh
~/.cargo/bin/cargo build -p tandem
~/.cargo/bin/cargo run -p tandem --example chat_ui_seed -- /tmp/tandem-ui-check
```

The folder argument is optional (by default the seed creates a new folder in the system's temporary folder) and must not exist yet. The seed prints the project and state paths and the launch command, `TANDEM_CLAUDE_OFFLINE=1 target/debug/tandem <project> --state-dir <state>`; run that from the repository root. With `TANDEM_CLAUDE_OFFLINE=1`, Tandem never starts the helper: the chat says `Claude is unavailable`, the buttons that need Claude are disabled, and Keep and Undo still work. The development bundle's `.tandem-local/m5/Tandem.app/Contents/MacOS/tandem` takes the same arguments.
