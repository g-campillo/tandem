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
2. **Open a project** with ⌘O or **Open Folder…** on the welcome screen, with **File → Recent Projects** (⌃R), or with a folder argument. The chat panel, on the right unless you swapped the panes, starts Claude (`Starting Claude…`).
3. **Sign in.** If Claude Code is not signed in, the chat shows `Sign in to Claude` with a command. Copy it, run it in Terminal (it is `claude auth login` for the copy of Claude Code inside Tandem), sign in with your Claude subscription, then press **Reconnect**. Tandem never sees your credentials; Claude Code keeps them.
4. **Settings** (⌘, or the gear at the bottom of the activity bar) shows the connection (`Connected · <subscription> · Claude Code <version>`), the Node.js and helper in use, the **Use my Claude Code setup** switch, the help level per language, the **Layout** switch, **Appearance** (window opacity and blur), **Fonts** (the font of each pane), the Rust and Python tools, and the recovery folder. Press ⌘, again or the close icon at the top of Settings to return to the code.

When Claude cannot be used, the chat says why and offers **Reconnect**: `Claude is unavailable` (for example Node.js or the helper is missing), `Claude subscription required` (Claude Code is set up for an API key or another provider; Tandem only works with a Claude subscription), or `Claude stopped`. If your plan's usage limit is reached, the turn ends with `Your Claude plan's usage limit was reached. Try again later.` Your work is kept in every case, and Tandem never switches to API billing.

**Use my Claude Code setup** is on by default: Claude then loads your Claude Code settings, `CLAUDE.md` files, MCP servers, skills and plugins, and the project's `.claude` folder, as running `claude` in the project would. Turn it off for projects you do not trust.

## Using the chat

- **Discuss.** Type in the composer at the bottom of the chat (⌘L focuses it; Enter or the arrow button sends, Shift+Enter adds a line). Claude reads your code and researches; short lines such as `↳ read src/cli.rs` show what it looked at. It does not change files.
- **Plan.** Claude proposes a plan card with the goal, the "done when" criteria and small steps. Ask for changes in the chat, then choose **I'll write it** (you write the code, Claude guides) or **You do it** (Claude builds it).
- **Guided build.** The active step is pinned above the composer. It shows what to do and why, the code (Beginner) or pseudocode (Intermediate), and explanations of its parts, shown when you point at a part. **Copy** copies the code or command, **Open file** opens the step's file (at the Beginner level at the line of the change, with the lines to replace or remove selected), and **Open Terminal** opens the terminal for a command step. Write the step, then press **Check my work** (⌥⌘C). A pass moves to the next step and Claude prepares its guidance; otherwise the verdict card lists what to fix, and why. Change the help level with `Help: <level> ▾` on the card or in Settings.
- **Takeover.** **Do it for me** lets Claude do the active step, and **Build the rest for me** every remaining step. Tandem first saves your unsaved files, because Claude edits the files on disk. While Claude works the editor is read-only and ⌘. stops it. Afterwards a card lists each file Claude created, modified or deleted, with **Keep** and **Undo** (and **Keep all**, **Undo all**). Undo cannot restore ignored files, such as anything in `target/` or `node_modules/`, or files over 2 MiB.
- **Permissions.** The **Permissions** button under the composer, or the same control at the right of the status bar, chooses **Ask** (the chat shows a command Tandem does not allow by itself with **Allow** and **Deny**), **Auto** (Claude Code asks only about risky actions) or **Bypass** (nothing asks, and Claude may edit project files in any turn; outside a takeover there is no Undo for that).
- **Tasks.** The task button in the chat header (the active task's title, with a ▾) lists the project's tasks and **New task** (also in the **Chat** menu). The **Model** and **Effort** buttons under the composer choose the Claude model and effort for the next message.
- **Stop.** While Claude works, the composer's arrow button becomes a square stop button. ⌘. or that button interrupts Claude at any time.

## Editor basics

Open a folder with ⌘O, **File → Recent Projects** (⌃R), or a folder argument; Tandem reopens the last project on the next start. Files open in tabs (at most 32), each with its own undo history while it is open. Unsaved changes are checkpointed in the recovery profile without saving the project file, and come back after a restart (`Recovered unsaved draft · Review before saving.`). **Compare with Disk** (⇧⌘D, or its icon at the right end of the tab bar) compares a buffer with the file on disk; **Use Disk Version** is explicit and undoable in that session. When a file changed on disk under your edits, your draft is kept for comparison, and **Save Copy** (⇧⌘S) saves it elsewhere. Files must be UTF-8 text up to 2 MiB.

The window follows VS Code's layout. The title bar holds the traffic lights, the command center (a search field in the middle) and, on the right, three switches that show and hide the file explorer, the panel below the editor and the chat. The activity bar on the window's edge beside the file tree has **Explorer**, **Search**, **Chat**, **Problems**, **Terminal** and, at the bottom, **Settings**; it moves with the file tree when the panes are swapped. Explorer and Search hide the file tree when it already shows their view; the others show and hide what they name. Tabs show a file icon, a dot for unsaved changes and a close button when you point at them, with **Save** and **Compare with Disk** at the right end of the tab bar; point at a tab to see the file's path. The breadcrumbs above the editor show the file's path and, with language tools on, the symbol the cursor is in. The status bar shows the git branch, the open file's errors and warnings (click for **Problems**), the cursor position (click for **Go to Line**), the line count, the language and the permission mode (click to change it). The file tree, the chat and the panel below the editor are floating panels with rounded corners; the editor sits flat between them. Point at an icon to see its name and, when it has one, its shortcut.

The panel below the editor has two tabs: **Terminal**, a basic interactive `/bin/sh` in the project folder with 2,000 lines of scrollback, started only when you open it, and **Problems**, the language tools' diagnostics and navigation results. The bin icon in the panel's header stops the terminal, and the close icon hides the panel (⌘J). Rust (rust-analyzer) and Python (Pyright) tools are enabled per project in Settings → **Language tools**, after a notice that they run with your user permissions; enable them only for projects you trust.

The file tree is on the left of the editor and the chat on the right. **View → Swap File Tree and Chat**, or the switch under Settings → **Layout**, puts the chat on the left and the file tree on the right; choose it again to swap back. Drag the divider between the editor and a pane to resize that pane, and double-click the divider to return the pane to its default width. The file tree can be 160 to 600 pixels wide and the chat 280 to 900, and the editor always keeps at least 360, so in a small window a pane can be narrower than you set it until the window grows again. Each pane keeps its width when the panes are swapped. The layout is the same for every project and is saved in the state folder, so it survives a restart.

With language tools on, rest the pointer on a name in the editor to see a tooltip: its signature, highlighted like the code in the editor, above its documentation. An error or a warning shows in the same tooltip, in a block of its own colour, as soon as the pointer is on the underlined text. The tooltip opens only while the pointer is on that text itself, never from the empty space beside a line, and closes when the pointer leaves it. Move the pointer onto the tooltip to scroll it or to select and copy its text (⌘C); a click or a scroll outside it, or any other key, closes it.

Tandem follows VS Code's macOS shortcuts. **Quick Open** is a dialog in the middle of the window: press ⌘P or click the command center, type part of a file's name and press Enter to open it. The command palette (⇧⌘P or F1) is the same dialog with `>` typed first: it lists every command with its shortcut; type to filter and press Enter to run one. The file explorer keeps its own filter field, which ⇧⌘E shows and focuses. The same commands are in the File, Edit, Selection, View and Go menus. ⌘/ comments with the marker of the file's language, placed before the first character of the line; with several lines selected it uses their smallest indentation, and it uncomments when every line is already commented. Sort Lines, Transform to Uppercase/Lowercase and Select Line have no shortcut and are run from the palette or the menus. Four keys differ from VS Code because Tandem already used them: ⌘L focuses the chat, ⌘. stops Claude, ⌥⌘C is Check my work and ⇧⌘D compares with disk. With several cursors, the line commands (comment, move, copy, delete) act on the first cursor only. Fold regions with the arrows in the gutter; ⌥⌘] and ⌘K ⌘J unfold.

| Action | Shortcut |
| --- | --- |
| Command palette / keyboard shortcuts | ⇧⌘P or F1 / ⌘K ⌘S |
| Open folder / recent projects / quick open / new file | ⌘O / ⌃R / ⌘P / ⌘N |
| Save / save all / save copy | ⌘S / ⌥⌘S / ⇧⌘S |
| Close file / close all / reopen closed file | ⌘W / ⌘K ⌘W / ⇧⌘T |
| Copy path / copy relative path / reveal in Finder | ⌘K P / ⇧⌥⌘C / ⌘K R |
| Find / replace / find next / find previous | ⌘F / ⌥⌘F / ⌘G / ⇧⌘G |
| Search the project | ⇧⌘F |
| Toggle line comment / block comment | ⌘/ / ⇧⌥A |
| Add / remove line comment | ⌘K ⌘C / ⌘K ⌘U |
| Move line up / down | ⌥↑ / ⌥↓ |
| Copy line up / down | ⇧⌥↑ / ⇧⌥↓ |
| Delete line | ⇧⌘K |
| Insert line below / above | ⌘↵ / ⇧⌘↵ |
| Indent / outdent line | ⌘] / ⌘[ |
| Join lines / trim trailing whitespace | ⌃J / ⌘K ⌘X |
| Add next occurrence / select all occurrences | ⌘D / ⇧⌘L |
| Add cursor above / below / at line ends | ⌥⌘↑ / ⌥⌘↓ / ⇧⌥I |
| Expand / shrink selection | ⌃⇧→ / ⌃⇧← (also with ⌘) |
| Go to line / definition / matching bracket | ⌃G / F12 / ⇧⌘\ |
| Back / forward | ⌃- / ⌃⇧- |
| Next / previous tab | ⌃Tab / ⌃⇧Tab (also ⌥⌘→ / ⌥⌘←, ⇧⌘] / ⇧⌘[) |
| Go to tab 1–9 | ⌘1 – ⌘9 |
| Next / previous problem | F8 / ⇧F8 |
| Trigger suggestions | ⌃Space |
| Show or hide the file explorer / show the explorer | ⌘B / ⇧⌘E |
| Show or hide the chat / focus the chat | ⌥⌘B / ⌘L |
| Show or hide the bottom panel / Terminal / Problems | ⌘J / ⌃` / ⇧⌘M |
| Word wrap / full screen | ⌥Z / ⌃⌘F |
| Unfold / unfold all | ⌥⌘] / ⌘K ⌘J |
| Compare with disk | ⇧⌘D |
| Editor text size, for all projects | ⌘= / ⌘− |
| Format buffer / references / document symbols | ⇧⌥F / ⇧F12 / ⇧⌘O |
| Check my work | ⌥⌘C |
| Stop Claude | ⌘. |
| Settings | ⌘, |

## Appearance and fonts

Settings (⌘,) → **Appearance** has two sliders. The window changes while you drag.

- **Window opacity** (30–100%, 100 by default) fades the whole window, text, panels and popups included, so you can see what is behind it. It stops at 30% so the window cannot disappear. The traffic lights do not fade.
- **Background blur** (0–100, 50 by default) softens what shows through the window. At 0 the blur is off and what is behind the window shows sharp. If your Mac does not let Tandem set the blur strength, Settings says `This Mac does not let Tandem set the blur strength, so Tandem uses the standard macOS blur: any strength above 0 turns it on.`

Settings → **Fonts** has one block per pane. Each font list shows every installed font and can be searched; sizes are whole pixels from 10 to 30.

- **Editor:** font (FiraCode Nerd Font by default), size (14) and ligatures (on).
- **Terminal:** font (FiraCode Nerd Font Mono), size (12) and ligatures (on).
- **Chat:** text font for messages and cards (the system font), code font for code blocks, inline code and the step card's code (FiraCode Nerd Font), one size (13; code is drawn one pixel smaller) and ligatures in code (on).
- **File explorer:** font (the system font) and size (13).

Ligatures draw pairs such as `->`, `!=` and `>=` as one sign in fonts that have them, such as FiraCode; the switch turns the OpenType features `calt` and `liga` on or off, as VS Code does. A saved font that is no longer installed is replaced by Menlo for code (the editor, the terminal and chat code) and by the system font for text (chat text and the file explorer). Settings then shows a warning such as `FiraCode Nerd Font is not installed, so the editor uses Menlo.`, and the font list shows `<font> (not installed)`.

⌘= and ⌘− change the editor's text size in every project, and the editor's size slider under Fonts moves with them. The text size a project had before is no longer used.

The appearance and the fonts are the same for every project. They are saved in the state folder with the other preferences, so they survive a restart.

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

`chat_ui_seed` writes a small Rust project and a state folder with five tasks, so the chat panel can be checked by hand without Claude and without using your subscription:

- `Add a --json flag to the CLI`: a conversation and a plan card in the Plan phase;
- `Add a --json flag to the CLI (guided)`, selected at start: a guided build at step 1 of 4 with stored guidance at all three help levels (the Beginner one inserts after a line);
- `Add a --json flag to the CLI (Claude did step 1)`: a takeover's three changed files waiting for Keep or Undo;
- `Reword the name help text (update a line)`: Beginner guidance that replaces one line, for checking the selection **Open file** makes;
- `Run the tests (a command step)`: Beginner guidance for a command, for checking explanations on a command.

```sh
~/.cargo/bin/cargo build -p tandem
~/.cargo/bin/cargo run -p tandem --example chat_ui_seed -- /tmp/tandem-ui-check
```

The folder argument is optional (by default the seed creates a new folder in the system's temporary folder) and must not exist yet. The seed prints the project and state paths and the launch command, `TANDEM_CLAUDE_OFFLINE=1 target/debug/tandem <project> --state-dir <state>`; run that from the repository root. With `TANDEM_CLAUDE_OFFLINE=1`, Tandem never starts the helper: the chat says `Claude is unavailable`, the buttons that need Claude are disabled, and Keep and Undo still work. The development bundle's `.tandem-local/m5/Tandem.app/Contents/MacOS/tandem` takes the same arguments.
