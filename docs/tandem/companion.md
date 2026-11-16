# The Claude chat companion

Tandem is a code editor with one Claude chat beside the code. In that chat you discuss a feature with Claude, agree on one plan, and then build it step by step. You can write each step yourself with Claude's guidance at the help level you choose, or hand the current step, or all the remaining steps, to Claude. Claude runs through Claude Code on your own Claude subscription.

This document describes the design as the code implements it. Setup and day-to-day use are in [local-use.md](local-use.md); the project overview is in the [README](../../README.md).

## The flow

A task moves through four phases. The chat header shows the first three as a trail, `Discuss › Plan › Build`, with the current one emphasised; when the task is done, all three are finished.

```
Chat · Claude              Discuss › Plan › Build
─────────────────────────────────────────────────
You  Add a --json flag to the CLI
Claude
  ↳ read src/cli.rs
  ↳ searched "clap bool flag"
  Should --json also change error output?
┌ Plan ─────────────────────────────────────────┐
│ Goal: Print results as JSON when --json is set│
│ Done when: output parses as JSON; old output  │
│            unchanged                          │
│ 1. Add `json` to Args      (src/cli.rs)       │
│ 2. Print JSON in run()     (src/main.rs)      │
│ 3. Add a test              (tests/cli.rs)     │
│     [ I'll write it ]   [ You do it ]         │
└───────────────────────────────────────────────┘
┌ Step 1 of 3 · Edit src/cli.rs  Help: Beginner ▾┐
│ Add a `json` flag field to the Args struct.   │
│ Why: clap turns each field into a flag.       │
│   #[arg(long)]                                │
│   json: bool,                         [Copy]  │
│ [Open file] [Check my work] [Do it for me]    │
│ [Build the rest for me]                       │
└───────────────────────────────────────────────┘
[ Message Claude…                              ]
  Model ▾   Effort ▾                       Send
```

Each project has its own list of tasks. The task button in the chat header lists them as `<title> · <phase>` and offers `New task` (also in the **Chat** menu). A task is titled after your first message, and after the plan's goal once there is a plan. `New task` does nothing while the current task is still empty, and you cannot switch tasks while Claude is working.

### Discuss

Describe what you want in the composer (Enter sends, Shift+Enter adds a line). Claude researches before it answers: it reads the project's code, searches the web when facts may have changed, and asks clarifying questions in the chat. What it does shows in the transcript as short activity lines such as `↳ read src/cli.rs`, `↳ searched "clap bool flag"` or ``↳ ran `cargo test` ``. Claude does not change files in this phase.

A message that starts with `/` goes to Claude Code exactly as typed, so Claude Code's slash commands and your skills keep working.

### Plan

When you and Claude agree on what to build, Claude puts a plan card in the chat: the goal, the "done when" criteria, and small numbered steps. Each step does one thing: create one file, make one focused edit to one file, or run one terminal command (a command is always its own step). Tandem checks the plan before showing it, for example that every file step names a path inside the project, and sends Claude the problem to fix when it does not.

To change the plan, say so in the chat; Claude sends the whole revised plan and the new card replaces the old one. The latest plan card has two buttons while Claude is idle and connected:

- **I'll write it** starts a guided build: you write the code, Claude guides each step.
- **You do it** hands the whole plan to Claude (a takeover of all the steps, described below).

Once Build has started the plan is fixed. If Claude needs a different plan, it says so, and you start a new task.

### Build, guided

The active step is pinned above the composer as a step card: `Step <n> of <total> · <Create|Edit|Run> <target>`, with a `Help: <level> ▾` button. Claude prepares the guidance for the step (`Preparing this step…`); the card then shows:

- a one-line summary and `Why:`, the reason for the change;
- the code (with **Copy**) at the beginner level, or pseudocode at the intermediate level;
- for a command step, the exact command with **Copy** and **Open Terminal** (opens the terminal at the bottom of the window);
- a breakdown that explains the parts of the code or command, one row per part.

The card's buttons:

- **Open file** (file steps) opens the step's file, as a new buffer if it does not exist yet, with the cursor on the line where the code goes when Claude named one.
- **Check my work** (⌥⌘C) asks Claude to inspect your work on this step. Claude may run build, test and lint commands for this. Files with unsaved changes are named in the request, and Claude is told to read them from the editor; commands only see saved files. The verdict appears as a card: `Step <n> looks right`, or `Step <n> needs changes` with each finding as `<path>:<line>  <problem>`, its reason and a hint. A finding with a path opens the file at that line. A pass moves the plan to the next step, and Claude prepares its guidance without being asked.
- **Do it for me** and **Build the rest for me** hand the work to Claude (see takeover below).
- **Guide me through this step** appears when the card has no guidance (for example after a Stop) and asks Claude again.

You can keep chatting during the build. Claude answers your questions about the active step but does not edit files.

After the last step, Claude checks the whole task against the "done when" criteria, again without being asked. A pass shows `The task meets its "done when" criteria` and the task is done. Otherwise the card says `Not done yet` with what is missing, and a card `All steps are finished` offers **Check again** and **Fix it for me**.

### Build, takeover

A takeover lets Claude do the work itself, by editing files and running commands in the project. There are two scopes:

- **Do it for me** (step card): only the active step. Claude does it, records it as done, and stops; the plan moves to the next step and Claude prepares its guidance, so you continue writing.
- **Build the rest for me** (step card), **You do it** (plan card) and **Fix it for me** (final card): every remaining step, in order. When the steps are done, Claude checks the "done when" criteria, fixes what fails, and reports the result. If Claude needs a decision from you, it stops and asks in the chat; your answer lets it continue the same takeover.

A note appears in the transcript as Claude finishes each step (`Step <n> done: …`). Before Claude starts, Tandem saves your unsaved buffers, because Claude edits the files on disk. It closes new tabs that were never saved and are still empty, so no empty files are created. It does not start when a save is still running, when an unsaved buffer's file changed on disk, or when an unsaved buffer is read-only; the status bar says why. A note in the chat lists the files Tandem saved. While Claude works, the editor is read-only, saving is paused, and the toolbar shows `Claude is editing files · ⌘. stops`.

When the takeover turn ends, a card `Claude changed <n> file(s)` lists every file that differs from before, each marked `Created`, `Modified` or `Deleted`:

- **Keep** accepts Claude's version of that file. **Undo** restores the file as it was before the takeover: a modified file gets its old content and permissions back, a deleted file returns, a created file is removed. **Keep all** and **Undo all** act on every file you have not decided yet.
- Keep and Undo work even when Claude is not connected, but not while Claude is working.
- Undo refuses, and leaves the file alone, when the file changed again after Claude's edit or has unsaved changes in the editor. Save or close it first.
- When you undo every file of a takeover and keep none, the plan goes back to the step where the takeover started, and Claude, when connected, prepares that step's guidance again.
- Starting another takeover keeps every earlier change you had not decided on, and the chat says so (`Earlier changes from Claude were kept.`).

Stop (⌘.) during a takeover ends it at once. The files Claude changed so far are listed for Keep or Undo, the task returns to the guided build, and nothing else starts by itself.

#### What Undo can restore

Before the takeover starts, Tandem takes a snapshot of the project's files; when it ends, it compares the project with the snapshot. Because Tandem compares the files themselves, changes made by shell commands are listed too. The snapshot covers the project's files that are not ignored:

- Files that are ignored (by `.gitignore` or a similar ignore file) and the folders `.git`, `target`, `node_modules`, `.tandem-local`, `.venv` and `__pycache__` are not tracked, so Undo cannot restore them. The changes card says so in its last line.
- Files up to 2 MiB are stored and can be restored. Larger files are tracked, so a change to one is listed, but its **Undo** is disabled (`Too large to restore`). Files over 64 MiB are not tracked at all. Each snapshot stores at most 256 MiB; files beyond that are tracked but cannot be restored.
- A project with more than 20,000 files that are not ignored cannot be taken over, because the snapshot would be incomplete.

Snapshots live in the Tandem state folder under `takeover/<id>/` and are removed once every file of that takeover is decided. They survive a restart: if Tandem closes during a takeover, the next start lists what Claude changed for review and notes `Tandem closed during a takeover. Review what Claude changed.`

### Done

A finished task shows `Task complete.` and all three phases as finished. You can still chat with Claude about it; Claude no longer edits files in it. Start a new task for the next feature.

## Help levels

Every guided step uses one of three help levels. The level is saved per language, and the step card's `Help: <level> ▾` button or Settings (⌘,) → **Help level per language** changes it. The default is Beginner.

| Level | File step | Command step |
| --- | --- | --- |
| Beginner | The complete code, and a breakdown of every keyword, construct and function call in it | The exact command, and a breakdown of every token |
| Intermediate | A description and pseudocode, not the finished code; the breakdown covers only what is new or unusual in that language | The exact command; the breakdown covers only the unusual parts |
| Expert | A short description and the reason | The exact command, a short description and the reason |

At every level Claude must say why: guidance without a reason is refused and sent back to Claude. Tandem checks each level's shape too (for example, Intermediate guidance that contains finished code is refused), so Claude cannot quietly give more or less help than you asked for.

A step's language comes from its file's extension (`.rs` Rust, `.go` Go, `.ts`/`.tsx` TypeScript, `.js`/`.mjs` JavaScript, `.py` Python, `.java` Java, `.sh` Shell), or from the language Claude names for a command step. Settings lists those seven languages plus any other language that has a saved level. Guidance is stored per step and level: switching to a level you already had shows the stored guidance at once, and a new level asks Claude for new guidance.

## What Claude may do

Tandem runs every Claude turn under one of four policies. Discussing and planning use **discuss**; questions and guidance during the build use **guided**; **Check my work** and the final check use **check**; a takeover uses **takeover**.

| | discuss, guided | check | takeover |
| --- | --- | --- | --- |
| Read code, search the project and the web | Yes | Yes | Yes |
| Read-only shell commands | Yes | Yes | Yes |
| Build, test and lint commands | Asks you | Yes | Yes |
| Edit files | No | No | Inside the project only |
| Other shell commands | Asks you | Asks you | Claude Code's rules decide |
| Other tools (for example your MCP servers) | Claude Code's rules decide | Claude Code's rules decide | Claude Code's rules decide |

- **Reading and research are always allowed.** Read-only shell commands are commands such as `ls`, `cat`, `head`, `grep`, `rg`, `find` (without actions such as `-delete` or `-exec`), `git status`, `git diff`, `git log`, `git show`, and `cd` into a subfolder, also chained with `&&` or piped into `head`, `tail`, `grep`, `rg`, `wc`, `sort` or `uniq`. Tandem's rule does not check the paths these commands read, so they can also read files outside the project.
- **Build, test and lint commands** run without asking during Check my work and a takeover. They include `cargo build`, `cargo check`, `cargo test`, `cargo clippy` and `cargo fmt --check`; `go build`, `go test` and `go vet`; `npm test` and `npm run test|lint|build|typecheck|check`; `tsc --noEmit`; `pytest` and `python -m pytest|unittest`; `ruff check`; `mypy`; `mvn test`; `gradle test`; and `make test|check|lint|build`. A command that would rewrite files (`--fix`, `--write`, `-w`, `cargo fmt` without `--check`) or uses redirection (other than `2>&1`), `;`, `$(…)` or backticks is never treated as read-only or as a build command. The exact rules are in `crates/tandem-claude/helper/src/policy.mjs`.
- **Edits happen only during a takeover, and only inside the project folder.** Outside a takeover, Claude Code's `Edit`, `Write` and `NotebookEdit` tools are refused with a message that tells Claude to explain the change instead. During a takeover, a path that resolves outside the project (also through a symlink) is refused. Edits inside the project are accepted without a card, and the changes card lets you undo them.
- **Other shell commands ask you** with an Allow/Deny card in the chat that shows what Claude wants to do and the exact command. Stop also denies a pending card. There is no "always allow" button.
- **Claude Code's own rules decide the rest:** shell commands during a takeover that are neither read-only nor build commands, and tools Tandem has no rule for, such as those of your MCP servers. These rules include the allow and deny rules in your Claude Code settings when **Use my Claude Code setup** is on. Whatever they would ask you about shows the same Allow/Deny card.

Tandem never uses Claude Code's `bypassPermissions` mode. Claude Code's `AskUserQuestion`, plan mode and worktree tools are switched off, because Tandem's chat and plan card replace them.

## Subscription and privacy

Tandem uses your Claude subscription through Claude Code, and nothing else:

- **No token handling.** Claude Code signs itself in (`claude auth login`) and keeps its own credentials. Tandem never reads, stores, logs or forwards OAuth tokens, anything from Claude Code's credentials in `~/.claude`, or the Keychain. It only looks at the account summary Claude Code reports: provider, subscription type and where an API key would come from.
- **Scrubbed environment.** Tandem starts the helper, and the helper starts Claude Code, without `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` or any variable that starts with `CLAUDE_CODE_USE_`, so a turn cannot fall back to API billing or another provider.
- **First-party subscription only.** A session opens only when Claude Code reports the first-party Anthropic provider, a subscription type, and no API key. Claude Code's start-up message is checked too: if it names an API key, the turn stops and the session is refused. Otherwise the chat shows `Claude subscription required` with the reason; there is no API-key field and no fallback. When Claude Code is signed out, the chat shows `Sign in to Claude` with the command to run and a **Reconnect** button.

What Claude reads while it works (project files, unsaved buffers it asks for, command output, web pages) becomes part of the conversation, as in any Claude Code session. Tandem's own copy of the chat stays in its local database.

### Use my Claude Code setup

Settings (⌘,) → **Claude** has a switch, **Use my Claude Code setup**, on by default:

- **On:** Claude Code loads your user settings (`~/.claude/settings.json`), the project's `.claude/settings.json` and `.claude/settings.local.json`, `CLAUDE.md` files, MCP servers, skills and plugins, as running `claude` in the project folder would.
- **Off:** Claude Code loads no settings files and no `CLAUDE.md`, and uses no MCP server but Tandem's own tools.

Turn it off for projects you do not trust. A project's `.claude` folder and `.mcp.json` can add hooks that run shell commands, MCP servers, and permission rules that let commands run without a card, and its `CLAUDE.md` steers what Claude does. Changing the switch reopens the task's Claude session; it is disabled while Claude is working.

## Model and effort

The composer has a **Model** button and, when the selected model supports effort levels, an **Effort** button. The model list is the one Claude Code reports for your account when the session opens. `Default` means Claude Code's default model, and `Effort: Default` means no effort override. Choices are saved for all projects and apply from the next message; nothing restarts. Choosing a model that does not support the saved effort clears the effort. If a saved model is no longer offered, Tandem switches back to Default and says so.

## Sessions and resume

Each task has its own Claude Code session. Only the active task's session is open; switching tasks closes it and resumes the selected one. Restarting Tandem, **Reconnect**, and switching **Use my Claude Code setup** resume the session too, so Claude remembers the conversation.

If Claude Code can no longer find the session, Tandem starts a new one and opens your next message with a short recap: the goal, the plan's steps marked done or active, and the last six messages. Tandem's own transcript is not affected.

**Stop** (⌘. or the composer's **Stop** button) interrupts the running turn and notes `Stopped.` in the chat. A turn interrupted by a restart or a crash is never replayed.

## Architecture

```
crates/tandem (GPUI app)      chat panel, step card, Settings
   │  Command ▲ Signal (state snapshot changed, questions about open buffers)
   ▼         │
crates/tandem-claude          Companion controller on its own thread
   │                            └ tandem-core: chat store (SQLite), takeover snapshots
   │  wire protocol v1: newline-delimited JSON over stdio
   ▼
crates/tandem-claude/helper   Node helper (plain JavaScript ES modules, no build step)
   │  @anthropic-ai/claude-agent-sdk 0.3.325
   ▼
Claude Code 2.1.325 (bundled with the SDK) → your Claude subscription
```

**The helper** (`crates/tandem-claude/helper`) wraps the Claude Agent SDK. It keeps one long-lived SDK session per task in streaming-input mode, so the same session serves discuss, guided, check and takeover turns. A `PreToolUse` hook applies the policy table above on every tool call (an edit is refused outside a takeover even though tools are fixed when the session starts), and the SDK's `canUseTool` callback forwards everything that needs approval to Tandem as a permission request. Tandem's own tools are an in-process MCP server named `tandem`:

| Tool | What it does |
| --- | --- |
| `propose_plan` | Shows the plan card (goal, "done when" criteria, steps) |
| `show_step` | Shows guidance for the active step at the help level Tandem named |
| `report_check` | Reports the verdict on one step or on the whole task |
| `step_done` | Records a finished step during a takeover |
| `read_editor_buffer` | Reads a file as it is in the editor, including unsaved changes |

Each tool call is forwarded to Rust and answered there, so validation lives in one place and Claude gets an error it can correct. The helper's modules are `frames` (framing), `env` (scrub), `auth` (account gate), `policy`, `activity` (transcript labels), `tools`, `session` and `host`; `main.mjs` is the entry point and writes only protocol frames to stdout.

**Wire protocol v1.** One JSON object per line, UTF-8, at most 8 MiB per line, each with `"v":1` and a `"type"`.

| Direction | Frames |
| --- | --- |
| Tandem → helper | `hello`, `open_session` (folder, session id or the id to resume, setup switch, model, effort, system prompt addition), `turn` (text, policy, model, effort), `interrupt`, `close_session`, `shutdown`, and `response` (the answer to a tool or permission request) |
| helper → Tandem | `reply` to each command (`ok` with data, or an error code: `bad_request`, `no_session`, `busy`, `auth_refused`, `resume_failed`, `spawn_failed`, `internal`); `event`: `turn_started`, `text_delta`, `assistant_text`, `activity`, `notice`, `auth_refused`, `permission_cancelled`, `turn_finished` (`completed`, `interrupted` or `error`), `session_closed`; `request`: a `tool` call or a `permission` question |

The reference conversation in `crates/tandem-claude/helper/test/fixtures/contract.jsonl` is replayed against the helper by the Node tests and decoded by the Rust tests, so both ends agree.

**`tandem-claude`** (Rust) finds Node (`PATH`, then nvm installs from the newest, then `/opt/homebrew/bin/node` and `/usr/local/bin/node`) and the helper, spawns the helper in its own process group with the scrubbed environment, and reads its frames (`install.rs`, `protocol.rs`, `client.rs`). The `Companion` controller (`companion.rs`, `guided.rs`, `takeover.rs`) runs on its own thread. It owns the helper and the task state, saves through the shared store, and publishes an immutable `ChatState` snapshot that the panel renders. The UI drives it only with commands (send, stop, choose guided, check my work, take over, keep, undo, and so on). `prompts.rs` holds the text Tandem adds to Claude Code's system prompt and each turn's message. `view.rs` holds the panel's decisions (which buttons show, the step card, the changes card) as pure functions, so they are tested without a GUI. Every controller test drives the scripted `tandem-fake-helper` binary instead of Node and Claude.

**`tandem-core`** holds the domain and storage: `chat` (tasks, plans, guidance, checks, change sets, preferences, and the rules that validate plans and guidance), the chat store, and `takeover` (snapshot, diff, keep, undo).

**Storage.** The state folder holds `workspace.sqlite` at schema 6. Next to the editor's tables (`workspace`, `draft`, `settings`, `project_tools`), schema 6 adds `chat_task` (one row per task with its phase, mode, plan, current step and session id), `chat_selection` (the selected task per project), `chat_item` (the transcript), `chat_guidance` (guidance per task, step and level) and `chat_change_set` (each takeover's files and decisions). Preferences are saved in `settings` under `chat_prefs`. Upgrading an older database first writes a backup next to it; tables from the earlier Codex-based design stay in an upgraded database untouched, and their tasks are not shown.

## Environment switches

| Variable | Effect |
| --- | --- |
| `TANDEM_CLAUDE_HELPER_DIR` | Development only: use the helper in this folder instead of `crates/tandem-claude/helper`. A packaged app always uses its bundled helper. |
| `TANDEM_CLAUDE_OFFLINE=1` | Never start the helper. The chat shows `Claude is unavailable` and everything that needs Claude is disabled; the editor, Keep and Undo still work. Used for checking the UI by hand without using the subscription. |

## Known limits

- One window, one active task and one Claude turn at a time; nothing runs in the background.
- The plan is fixed once Build starts; a different plan needs a new task.
- Undo does not cover ignored files, cannot restore files over 2 MiB, and does not track files over 64 MiB. A project with more than 20,000 files that are not ignored cannot be taken over.
- The editor is read-only and saving is paused while Claude edits files.
- Read-only shell commands are allowed without a card even when they read outside the project.
- Permission cards have no "always allow"; Claude Code's sandbox option is not used.
- Commands that Claude runs see saved files only; unsaved buffers reach Claude only through `read_editor_buffer`.
- The text you have typed but not sent is not saved.
- Tasks from the earlier Codex-based design are not shown.
- macOS on Apple Silicon only. A packaged Tandem contains Anthropic's proprietary Claude Code binary, is for local use on your own Mac, and must not be redistributed; see [third-party-notices.md](third-party-notices.md).
