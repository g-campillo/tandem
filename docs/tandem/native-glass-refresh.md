# Native glass refresh verification

Verified on macOS 27.0.1 (26A434), Apple silicon, on 2026-11-14. The final QA found no additional visual defect requiring a source change. Task 5 adds this report only; the application source remains at `a0fd795b6ef691d2d9542e86650560c28cb57a36`.

## Implemented design

Application-owned light/dark semantic colors give toolbar, file navigation and utility navigation a restrained translucent tint over native macOS material. Editor, terminal, panel bodies and long-form content remain opaque. The standard titlebar, traffic lights, native menus and dialogs remain in use. This uses the pinned GPUI Kit 0.9.1 native blur, without a framework migration or a refraction shader.

The application applies its palette after toolkit theme loading and observes window appearance changes. System UI typography, quiet blue selection, line icons, 6px controls, 10px cards and subtle separators establish hierarchy. Chrome uses 13px labels, panels 14px prose, metadata 12px and section headings 16px; the 16px rem basis is unchanged. Editor font-size state and terminal 12px monospace/18px line-height/nine-row behavior are retained. Pane widths interpolate from 210px/320px at 900px content width to 250px/390px at 1280px. Controls wrap, long prose and paths remain available, and code-like paths/output have horizontal scroll viewports.

Labels, handlers, shortcuts, visibility gates and workflows are preserved by the accepted Tasks 1–4 reviews. Solution visibility, edit authority, model assessment, independent check evidence, manual completion, scope preparation, authorization and import/undo remain distinct.

## Automated results

All commands below ran from `/Users/gabriel.campillo/code/tandem` against the final application source. There are no new tests in Task 5 because there is no code change or newly discovered regression to cover.

| Command | Observed result |
| --- | --- |
| `/Users/gabriel.campillo/.cargo/bin/cargo test -p tandem appearance` | 6 passed |
| `/Users/gabriel.campillo/.cargo/bin/cargo test --workspace` | 75 passed, 2 existing intentional ignores; no failures |
| `python3 -m unittest discover -s scripts/tests` | 7 passed |
| `/Users/gabriel.campillo/.cargo/bin/cargo fmt --all -- --check` | Passed |
| `/Users/gabriel.campillo/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `/Users/gabriel.campillo/.cargo/bin/cargo build -p tandem` | Passed |
| `git diff --check` | Passed after this report was written |

The workspace test command emits the pre-existing duplicate example-output-name warnings for `integration` and `python`. The ignores remain the opt-in managed-runtime transport-loss test and the subprocess crash-writer helper.

The six ordinary appearance tests cover light/dark text and focus contrast, translucent chrome composited over black/white endpoints, control hover/active colors, opaque content projection after toolkit theme reloads, pane interpolation/clamping and terminal available width. They establish palette/configuration and geometry behavior, not native observer delivery or rendered accessibility. The Task 3 standalone pinned-Taffy reproduction established a 1200px immediate child and 912px scroll range in a 288px viewport after its fix; it is prior sizing evidence, not a new Task 5 run or a native rendering test.

GPUI Kit test-support cannot resolve against the pinned runtime dependencies without Taffy/unicode-properties downgrades. No dependencies were changed and no unsupported TestAppContext/offscreen harness, snapshot goldens or image-diff system was added.

## Native observation records

Screenshots were displayed and inspected through CUA. No PNG files were saved. Sizes below are observed outer window image sizes; the configured content defaults/minimum remain 1280×820 and 900×600. Each record identifies its source commit and disposable fixture. Prior records are retained from the accepted task handoffs and were not gratuitously repeated.

All fixture paths below are relative to `.tandem-local/native-glass/` in this checkout:

| Record | Source commit | Fixture | Appearance and size | Inspected surfaces |
| --- | --- | --- | --- | --- |
| D2 | `be9bc641615a60d4ae3d9ec5df890e3ea7c57dfe` | `qa-task2-y8hn_asr` | Native current Dark; 1280×852 and approximately 902×632 | Workspace/editor/sidebar/terminal, wrapped toolbar/status, expanded Recent with eight long paths; welcome scrolling was previously checked at 900×852 |
| D3 | `394b01b4d826edff1ba53bae92cc93445b3c49ec` | `qa-task3-iwy1krpq` | Native current Dark; default Checks and all seven tabs at approximately 902×632 | Tools, Problems, Checks, Task, App, Companion and Takeover navigation; failed/truncated synthetic receipt, scope text and long path/output tails |
| D4 | `a0fd795b6ef691d2d9542e86650560c28cb57a36` | `qa-task4-d6xz241k` | Native current Dark; 1280×852 and approximately 902×632 | Long task/steps, preferences, plan editing, guidance, review and improvement; visible/collapsed solution; prepared scope, check envelope and staged-diff presentation |
| L5a | `a0fd795b6ef691d2d9542e86650560c28cb57a36` | `qa-task5-light-yovxuj4s` | Per-app forced Light startup; 1280×852 | Empty editor/Companion, long task/step, sidebar/file selection, syntax, find match, replace controls and native Save Copy dialog |
| L5b | Same as L5a | Same as L5a | Per-app forced Light; approximately 902×632 | Editor plus terminal, wrapped toolbar and long status; all seven utility tabs, Tools scrolling, Task, App, prepared Takeover scope/check warning, expanded preferences, historical response/solution and focus ring |
| L5c | Same as L5a | Same as L5a | Per-app forced Light; approximately 902×632 | Welcome before/after vertical scrolling: the fifth long recent-project path became fully reachable, with status and utility navigation still visible |

These are representative observations across the intended four-way appearance/size matrix, not an exhaustive pass of every state in every cell. The source did not change after D4, so its Dark observations remain applicable. Light checks use a per-app startup override; they do not verify native system Light selection or live OS switching.

## State and behavior coverage

| Evidence class | Observed check | Result and boundary |
| --- | --- | --- |
| Native manual, real local editing | L5 find `double`, replace with `twice`, undo, save and Save Copy | Match count/highlight and replacement were visible; undo restored the source. Byte checks confirmed original `src/sample.rs` and the saved copy match. Native filename entry produced `sample.rssample-copy`; this was retained as the disposable copy. No user project was edited. |
| Native manual, real terminal | L5 minimum-width terminal | `printf 'Light terminal α→✓\n'` produced readable Unicode output; terminal controls and output fit alongside both panes. D2 also covered scroll/latest/interrupt/hide/show/stop and nine-row controls. Clipboard contents were not independently verified. |
| Native manual, local UI | L5 welcome, toolbar, lists and panels | Minimum-height welcome scroll reached the fifth full path. Toolbar actions wrapped; long status wrapped without displacing the editor. Every utility tab and representative content was inspected at minimum size. No observed clipping required a fix. |
| Native manual, persisted synthetic content | D3 failed/truncated receipt | Failure/truncation and diagnostic link were displayed. Wheel-down over horizontal viewports reached `OUTPUT_FAR_SUFFIX` and interpreter/runtime path suffixes. CUA left/right gesture was inconclusive; direct horizontal trackpad behavior remains unverified. |
| Native manual, persisted synthetic content | D4/L5 task, responses and preferences | Long task/step and preference scope controls wrapped and scrolled. Historical review gaps and expected-versus-measured improvement warnings remained explicit. These are five offline Store-seeded response variants, not model results. |
| Native manual, persisted synthetic content | D4/L5 solution visibility | D4 typing into the read-only example did not alter it; collapsing removed its accessible content. L5 inspected the expanded Rust example, then switched guided language to Python: the solution editor/sentinel, Copy, Explain and Collapse nodes disappeared from AX, replaced by the policy/language warning. No model request or permission grant was invoked. Policy-change while expanded was not separately checked. |
| Native manual, real local preparation | D4/L5 Takeover | Exact writable paths, allowed check, command envelope, isolation limits and separate authorization action were visible and scrollable. Preparation captured disposable local source; authorization was never activated. D4 inspected the staged-diff view. Nonempty import and undo previews were not reached. |
| Native manual, limited accessibility/input | L5 focus and shortcuts | Quick-open focus and a keyboard-Tab focus ring on Terminal Interrupt were visible. Find, replace, undo, save and Save Copy shortcuts were exercised. A zoom-out changed persisted editor size to 13px; a paired attempted zoom-in was not independently established. This is limited focus/shortcut evidence, not complete keyboard traversal or spoken VoiceOver. |
| Native manual, offline state | D4/L5 connection and health | Offline/disconnected messaging and ordinary disabled controls were visible. Brief App runtime verification was observed; it does not establish AI running/stop behavior. No connection, sign-in or model action was run. |
| Prior native manual | D2 editing/recovery/close | Find/edit/selection/undo, unsaved file close, recovery, external disk conflict and diff were observed in disposable files. Those checks were not repeated for this documentation-only task. |

## Isolated Light launch and retained preview

The single fresh forced-Light attempt succeeded; initial CUA attachment took approximately 10.5 seconds. A unique QA-only app name, display name, identifier and path avoid ambiguity with earlier QA bundles. The wrapper contains an unchanged copy of `target/debug/tandem`, runtime resources copied with `scripts.macos_release.copy_runtime`, and an `APPL` Info.plist with executable/version/high-resolution keys and `NSRequiresAquaSystemAppearance=true`. No release packaging, signing or installation was performed.

Run: `.tandem-local/native-glass/qa-task5-light-yovxuj4s`.

```text
/usr/bin/open -n "/Users/gabriel.campillo/code/tandem/.tandem-local/native-glass/qa-task5-light-yovxuj4s/Tandem QA Task5 Light yovxuj4s.app" --args /Users/gabriel.campillo/code/tandem/.tandem-local/native-glass/qa-task5-light-yovxuj4s/project --state-dir /Users/gabriel.campillo/code/tandem/.tandem-local/native-glass/qa-task5-light-yovxuj4s/state --no-restore
```

The wrapper executable and debug binary SHA-256 matched: `40187e1bd4f9455121fe7ec83620097852c4f0e0b20a2cda5caa7fc975488997`. CUA selected the full bundle path only after this launch. The scratch Store helper reused the Task 4 fixture pattern at fresh paths, adding eight long recent roots; it called no model methods. The run's `fixture.json`, `fixture-src/`, project and state retain the exact launch metadata and synthetic fixture source.

The first owned process quit successfully. The same wrapper was then launched with only `--state-dir <same isolated state> --no-restore` for welcome QA, and a welcome recent-project button reopened the disposable project. At handoff, PID **75425** remains open for preview at approximately 902×632, showing the project editor and Companion in Light. Its exact bundle/project/state paths are those above; its bundle identifier is `dev.tandem.nativeglass.qa.qatask5lightyovxuj4s`. Only this QA process is owned by Task 5. Earlier native-glass QA processes were already stopped; unrelated existing m4/m5 apps were untouched. The QA run is ignored/untracked and is not part of the commit.

## Explicit unverified coverage

- Native live system appearance switching, actual system Light selection, contrasting-backdrop blur/refraction, Reduce Transparency, Increase Contrast and spoken VoiceOver. Global macOS settings were not changed. Palette tests, startup overrides and AX inspection do not prove these behaviors.
- Exhaustive hover/pressed/disabled rendering and keyboard traversal across every control; native utility-tab selection by keyboard; full menu/titlebar movement/zoom behavior. The standard titlebar and traffic lights were visible, and the native Save Copy dialog was used.
- Native completion, hover, diagnostic decorations and language-server behavior. Tools were left off rather than enabling local execution for visual QA. D3's fixture diagnostic link does not establish a live LSP result. Project-search/quick-open filtering, many-tab scrolling and zoom limits were not newly verified in Task 5.
- Live connected, quota/usage-limit, AI running/stop/cancellation and transient error states; busy authorization disabling; nonempty staged/import/undo previews. Safe persisted fixtures cannot establish these transient states, and starting model work or manufacturing runtime flags was outside scope. Existing deterministic/domain tests are additional evidence, not a native visual pass.
- Every long context-path and preference-policy combination, expanded solution hiding on policy changes, every check receipt variant and native Open Folder/create-save dialog journey. The recorded observations cover representative states only.

The installed application, existing recovery profiles, runtime configuration, credentials, global preferences, dependencies and source authority were not changed by this QA.
