//! Parameter hints: a card above the cursor with the signature of the call being typed and
//! its active parameter highlighted. They are asked for when a trigger character of the
//! language server is typed, by Trigger Parameter Hints (⇧⌘Space) and by a completion
//! entry that asks for them, and asked for again while the user types or moves the cursor
//! with the card open, until the server says the cursor is in no call.
use super::*;
use crate::appearance::{RADIUS_CARD, RADIUS_CONTROL, palette};
use gpui_kit::assets::IconName;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use lsp_types::SignatureHelp;
use std::time::Duration;
use tandem_lsp::features::signature;

/// The client command of a completion entry that asks for parameter hints.
pub(super) const TRIGGER_PARAMETER_HINTS: &str = "editor.action.triggerParameterHints";
/// How long open hints wait after an edit or a cursor move before they are asked for again.
const FOLLOW_DELAY: Duration = Duration::from_millis(80);
const CARD_MAX_WIDTH: f32 = 560.;
const DOCUMENTATION_MAX_HEIGHT: f32 = 200.;
/// Space between the cursor and the card.
const GAP: f32 = 4.;
/// The card keeps this far from the window's edges.
const EDGE: f32 = 8.;

/// The parameter hints of one editor: on screen once the server has described a call, or
/// waiting for its first reply.
pub(in crate::workspace) struct SignatureState {
    pub(super) editor_id: EntityId,
    /// The server's signatures; none while the first reply is awaited.
    help: SignatureHelp,
    /// The signature the user moved to with the card's arrows.
    chosen: Option<usize>,
    /// The request in flight, or waiting to go out; dropping it cancels it.
    request: Option<Task<()>>,
    /// The cursor when the hints were last asked for, so a move can be told from a redraw.
    cursor: usize,
    /// Tells this reply's documentation apart from an earlier one's.
    serial: u64,
}

impl SignatureState {
    /// Hints of `editor_id` that wait for their first reply.
    fn waiting(editor_id: EntityId) -> Self {
        Self {
            editor_id,
            help: SignatureHelp {
                signatures: Vec::new(),
                active_signature: None,
                active_parameter: None,
            },
            chosen: None,
            request: None,
            cursor: 0,
            serial: 0,
        }
    }

    /// Whether the card is on screen.
    pub(super) fn shown(&self) -> bool {
        !self.help.signatures.is_empty()
    }
}

/// A request for parameter hints on its way, with the text and the cursor it is about.
struct SignatureRequest {
    reply: Task<Result<Option<SignatureHelp>>>,
    text: String,
    cursor: usize,
}

/// What a request for parameter hints says about how it came about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Asked {
    /// The trigger character typed right before.
    trigger: Option<char>,
    /// Whether an edit or a cursor move asked.
    content_change: bool,
}

/// The user asked for the hints: ⇧⌘Space, or a completion entry they accepted.
const INVOKED: Asked = Asked {
    trigger: None,
    content_change: false,
};
/// The text or the cursor changed while the hints were open.
const CHANGED: Asked = Asked {
    trigger: None,
    content_change: true,
};

impl Workspace {
    /// Trigger Parameter Hints (⇧⌘Space): asks for the hints of the call at the cursor of
    /// the active file.
    pub(in crate::workspace) fn trigger_signature_help(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.ide_target() else {
            self.message("Open a file to use this command.", false, cx);
            return;
        };
        let Some(manager) = target
            .manager
            .as_ref()
            .filter(|manager| manager.health().state == "ready")
        else {
            self.message(
                "Parameter hints need a running language server for this file.",
                false,
                cx,
            );
            return;
        };
        if !offers_signatures(manager) {
            self.message(
                "This file's language server does not offer parameter hints.",
                false,
                cx,
            );
            return;
        }
        // From the menu bar the focus can be elsewhere; Escape closes the card in the editor.
        target
            .editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        self.ask_signature(target.editor.entity_id(), INVOKED, Duration::ZERO, cx);
    }

    /// Asks for the hints in `editor` because an accepted completion entry asked for them.
    pub(super) fn signature_after_accept(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        self.ask_signature(editor, INVOKED, Duration::ZERO, cx);
    }

    /// Follows a user edit in `editor` that typed `typed`: a trigger character of the
    /// language server (or, with the card open, a retrigger character) asks for the hints
    /// at once; any other edit while the hints are open or awaited asks again shortly.
    pub(super) fn signature_edited(
        &mut self,
        editor: &Entity<EditorState>,
        typed: Option<char>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let id = editor.entity_id();
        let open = self
            .ide
            .signature
            .as_ref()
            .filter(|hints| hints.editor_id == id);
        let shown = open.is_some_and(SignatureState::shown);
        let open = open.is_some();
        let trigger = typed.filter(|typed| {
            !self.ide_refused()
                && editor.read(cx).focus_handle(cx).is_focused(window)
                && self
                    .ide_target()
                    .filter(|target| target.editor.entity_id() == id)
                    .and_then(|target| target.manager)
                    .filter(|manager| manager.health().state == "ready")
                    .is_some_and(|manager| {
                        let features = manager.capabilities();
                        is_trigger(
                            *typed,
                            &features.signature_triggers(),
                            &features.signature_retriggers(),
                            shown,
                        )
                    })
        });
        if trigger.is_some() {
            let asked = Asked {
                trigger,
                content_change: open,
            };
            self.ask_signature(id, asked, Duration::ZERO, cx);
        } else if open {
            self.ask_signature(id, CHANGED, FOLLOW_DELAY, cx);
        }
    }

    /// After `editor` changed, asks for its open or awaited hints again shortly when the
    /// cursor moved.
    pub(super) fn signature_moved(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let cursor = editor.read(cx).cursor();
        if self
            .ide
            .signature
            .as_ref()
            .is_some_and(|hints| hints.editor_id == editor.entity_id() && hints.cursor != cursor)
        {
            self.ask_signature(editor.entity_id(), CHANGED, FOLLOW_DELAY, cx);
        }
    }

    /// Closes the hints of `editor` when the editor loses the focus.
    pub(super) fn signature_blurred(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        if self
            .ide
            .signature
            .as_ref()
            .is_some_and(|hints| hints.editor_id == editor.entity_id())
        {
            self.close_signature(cx);
        }
    }

    /// Handles Escape for the hints of the `focused` editor: closes them, and takes the key
    /// when the card was on screen. A request still awaited is cancelled as well.
    pub(super) fn signature_key(
        &mut self,
        key: IdeKey,
        focused: Option<EntityId>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(hints) = self
            .ide
            .signature
            .as_ref()
            .filter(|hints| key == IdeKey::Escape && Some(hints.editor_id) == focused)
        else {
            return false;
        };
        let shown = hints.shown();
        self.close_signature(cx);
        shown
    }

    /// Closes the hints when their editor is no longer the active tab's on screen.
    pub(super) fn signature_tick(&mut self, cx: &mut Context<Self>) {
        let active = self.ide_target().map(|target| target.editor.entity_id());
        if self
            .ide
            .signature
            .as_ref()
            .is_some_and(|hints| Some(hints.editor_id) != active)
        {
            self.close_signature(cx);
        }
    }

    /// Closes the card and cancels its request.
    fn close_signature(&mut self, cx: &mut Context<Self>) {
        if self.ide.signature.take().is_some() {
            self.signature_changed(cx);
        }
    }

    /// Tells the key listeners whether the card is on screen, and redraws.
    fn signature_changed(&mut self, cx: &mut Context<Self>) {
        self.ide.sync_keys();
        cx.notify();
    }

    /// Moves the card to the signature `delta` places from the one it shows, around the
    /// ends.
    fn choose_signature(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(hints) = self.ide.signature.as_mut() else {
            return;
        };
        let Some(view) = signature::signature_view(&hints.help, hints.chosen) else {
            return;
        };
        hints.chosen = Some(cycle(view.index, view.count, delta));
        cx.notify();
    }

    /// Asks for the hints of `editor` after `delay`, replacing a request in flight; the
    /// card on screen stays until the reply replaces or closes it.
    fn ask_signature(
        &mut self,
        editor: EntityId,
        asked: Asked,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(cursor) = self
            .ide
            .tabs
            .get(&editor)
            .and_then(|tab| tab.editor.upgrade())
            .map(|state| state.read(cx).cursor())
        else {
            return;
        };
        let request = cx.spawn(async move |view, cx| {
            if !delay.is_zero() {
                smol::Timer::after(delay).await;
            }
            let request = view.update(cx, |view, cx| view.signature_request(editor, asked, cx));
            let Ok(Some(request)) = request else {
                return;
            };
            let reply = request.reply.await;
            view.update(cx, |view, cx| {
                view.signature_replied(editor, &request.text, request.cursor, reply, cx)
            })
            .ok();
        });
        let mut hints = match self.ide.signature.take() {
            Some(hints) if hints.editor_id == editor => hints,
            _ => SignatureState::waiting(editor),
        };
        hints.request = Some(request);
        hints.cursor = cursor;
        self.ide.signature = Some(hints);
        self.signature_changed(cx);
    }

    /// The request for the hints at the cursor of `editor`, with the text and the cursor it
    /// is about. When it cannot be made (the editor is not the active tab's on screen, the
    /// editor refuses edits, or its language server is not ready or offers no hints) the
    /// hints close and nothing is asked.
    fn signature_request(
        &mut self,
        editor: EntityId,
        asked: Asked,
        cx: &mut Context<Self>,
    ) -> Option<SignatureRequest> {
        let target = self
            .ide_target()
            .filter(|target| target.editor.entity_id() == editor && !self.ide_refused());
        let manager = target
            .as_ref()
            .and_then(|target| target.manager.clone())
            .filter(|manager| manager.health().state == "ready" && offers_signatures(manager));
        let (Some(target), Some(manager)) = (target, manager) else {
            self.close_signature(cx);
            return None;
        };
        self.sync_language(cx);
        let (text, cursor) = {
            let state = target.editor.read(cx);
            (state.text().to_string(), state.cursor())
        };
        let shown = self
            .ide
            .signature
            .as_ref()
            .filter(|hints| hints.editor_id == editor && hints.shown())
            .map(|hints| shown_help(&hints.help, hints.chosen));
        let params = signature::signature_params(
            &text,
            cursor,
            asked.trigger,
            asked.content_change,
            shown.as_ref(),
        );
        let Ok(params) = params else {
            self.close_signature(cx);
            return None;
        };
        let request = cx.background_spawn({
            let (file, instance, text) = (target.file, target.instance, text.clone());
            async move {
                let reply = manager
                    .request(
                        "textDocument/signatureHelp",
                        &file,
                        &instance,
                        &text,
                        params,
                    )
                    .await?;
                anyhow::ensure!(manager.is_current(&reply.stamp), "Stale parameter hints");
                signature::signature_help(reply.value)
            }
        });
        Some(SignatureRequest {
            reply: request,
            text,
            cursor,
        })
    }

    /// Shows the server's reply about `text` with the cursor at `cursor` while the editor
    /// still has them; a reply about text the editor no longer has (an edit of Tandem's own
    /// came meanwhile) asks again. No call at the cursor closes the card; a failed request
    /// leaves the card as it was, or closes hints still awaited.
    fn signature_replied(
        &mut self,
        editor: EntityId,
        text: &str,
        cursor: usize,
        reply: Result<Option<SignatureHelp>>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .ide
            .tabs
            .get(&editor)
            .and_then(|tab| tab.editor.upgrade())
            .is_some_and(|state| {
                let state = state.read(cx);
                state.cursor() == cursor && *state.text() == text
            });
        let Some(hints) = self
            .ide
            .signature
            .as_mut()
            .filter(|hints| hints.editor_id == editor)
        else {
            return;
        };
        // This runs inside the request's own task; dropping its handle lets it finish.
        hints.request = None;
        if !current {
            self.ask_signature(editor, CHANGED, FOLLOW_DELAY, cx);
            return;
        }
        match reply {
            Ok(Some(help)) => {
                self.ide.hints += 1;
                hints.help = help;
                hints.chosen = None;
                hints.serial = self.ide.hints;
                self.signature_changed(cx);
            }
            Ok(None) => self.close_signature(cx),
            Err(_) if hints.shown() => {}
            Err(_) => self.close_signature(cx),
        }
    }
}

/// Whether `manager` answers textDocument/signatureHelp.
fn offers_signatures(manager: &LanguageManager) -> bool {
    manager.capabilities().has("signatureHelpProvider")
}

/// Whether typing `typed` asks for parameter hints: it is one of the server's trigger
/// characters, or, while the card is `shown`, one of its retrigger characters.
fn is_trigger(typed: char, triggers: &[String], retriggers: &[String], shown: bool) -> bool {
    let typed = typed.to_string();
    triggers.contains(&typed) || (shown && retriggers.contains(&typed))
}

/// The character typed by a single-cursor keystroke that inserted `inserted` at `start`
/// and left the cursor at `cursor`: the first character inserted, when the cursor is right
/// after it. That holds for one typed character and for an opening bracket the editor
/// closed at once (`(` inserts `()` with the cursor between them), and not for a paste.
pub(super) fn typed_char(start: usize, inserted: &str, cursor: usize) -> Option<char> {
    let typed = inserted.chars().next()?;
    (cursor == start + typed.len_utf8()).then_some(typed)
}

/// The hints on screen as the server is told about them: its reply, with the signature
/// the user `chosen` as the active one.
fn shown_help(help: &SignatureHelp, chosen: Option<usize>) -> SignatureHelp {
    let mut help = help.clone();
    if let Some(chosen) = chosen {
        help.active_signature = Some(chosen as u32);
    }
    help
}

/// The signature `delta` places from `index` among `count`, around the ends.
fn cycle(index: usize, count: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (index as isize + delta).rem_euclid(count as isize) as usize
}

/// The card of `hints` above the cursor of `editor`, or `None` while the cursor is not
/// laid out: the signature in the monospace font with the active parameter in the accent
/// colour, arrows and a counter when there are several signatures, and the parameter's
/// and the signature's documentation below as Markdown, styled like the hover card.
pub(super) fn signature_card(
    hints: &SignatureState,
    editor: &Entity<EditorState>,
    face: &Face,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let view = signature::signature_view(&hints.help, hints.chosen)?;
    let state = editor.read(cx);
    let cursor = state.cursor();
    let anchor = state.range_to_bounds(&(cursor..cursor))?;
    let p = palette(cx.theme().mode);
    let active = HighlightStyle {
        color: Some(p.accent),
        font_weight: Some(FontWeight::BOLD),
        ..HighlightStyle::default()
    };
    let label = StyledText::new(view.label.clone())
        .with_highlights(view.active.clone().map(|range| (range, active)));
    let arrow = |id: &'static str, icon: IconName, delta: isize| {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded(px(4.))
            .text_color(p.secondary_foreground)
            .hover(|arrow| arrow.bg(p.hover()).text_color(p.foreground))
            .child(Icon::new(icon).with_size(px(14.)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    // The editor keeps the focus, so the card stays open.
                    window.prevent_default();
                    cx.stop_propagation();
                    view.choose_signature(delta, cx);
                }),
            )
    };
    let counter = (view.count > 1).then(|| {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.))
            .text_size(px(12.))
            .text_color(p.secondary_foreground)
            .child(arrow("signature-previous", IconName::ChevronUp, -1))
            .child(format!("{} / {}", view.index + 1, view.count))
            .child(arrow("signature-next", IconName::ChevronDown, 1))
    });
    let signature_row = div()
        .flex()
        .items_start()
        .gap_2()
        .px(px(10.))
        .py_2()
        .rounded(px(RADIUS_CONTROL))
        .bg(p.content)
        .children(counter)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(cx.theme().mono_font_family.clone())
                .font_features(face.features.clone())
                .child(label),
        );
    let mut style = TextViewStyle::default()
        .paragraph_gap(rems(0.5))
        .heading_font_size(|_, base| base)
        .code_block(
            StyleRefinement::default()
                .bg(p.content)
                .p_2()
                .rounded(px(RADIUS_CONTROL))
                .text_size(px(12.)),
        );
    // The editor's highlight theme, so code has the editor's colours.
    style.highlight_theme = cx.theme().highlight_theme.clone();
    let serial = hints.serial;
    let parts: Vec<_> = [
        ("parameter", view.parameter_documentation.clone()),
        ("signature", view.documentation.clone()),
    ]
    .into_iter()
    .filter_map(|(part, text)| {
        let id = SharedString::from(format!("signature-{part}-{serial}-{}", view.index));
        Some(TextView::markdown(id, text?).style(style.clone()))
    })
    .collect();
    let documentation = (!parts.is_empty()).then(|| {
        div()
            .id(("signature-documentation", serial))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_2()
            .px(px(2.))
            .max_h(px(DOCUMENTATION_MAX_HEIGHT))
            .overflow_y_scroll()
            .children(parts)
    });
    let card = div()
        .id(("signature-card", serial))
        .occlude()
        .max_w(px(CARD_MAX_WIDTH))
        .flex()
        .flex_col()
        .gap_2()
        .p(px(10.))
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
        .shadow_lg()
        .font_family(cx.theme().font_family.clone())
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(p.foreground)
        .child(signature_row)
        .children(documentation)
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            // A press on the card leaves the focus, and so the card, in the editor.
            window.prevent_default();
            cx.stop_propagation();
        });
    Some(
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .attach(anchor, Anchor::TopLeft)
                .offset(point(px(0.), px(-GAP)))
                .snap_to_window_with_margin(px(EDGE))
                .child(card),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would bring in GPUI's `test` macro through `gpui_kit::*`
    // and shadow Rust's `#[test]`.
    use super::{cycle, is_trigger, shown_help, typed_char};
    use lsp_types::SignatureHelp;
    use serde_json::json;

    #[test]
    fn one_typed_character_is_the_typed_character() {
        assert_eq!(typed_char(9, "(", 10), Some('('));
        assert_eq!(typed_char(9, ",", 10), Some(','));
        assert_eq!(typed_char(0, "é", 2), Some('é'));
    }

    #[test]
    fn an_auto_closed_bracket_is_its_opening_character() {
        // `(` typed after `describe` inserts `()` with the cursor between them.
        assert_eq!(typed_char(8, "()", 9), Some('('));
    }

    #[test]
    fn a_paste_or_a_deletion_types_nothing() {
        assert_eq!(typed_char(8, "(x", 10), None);
        assert_eq!(typed_char(8, "()", 10), None);
        assert_eq!(typed_char(8, "", 8), None);
    }

    #[test]
    fn trigger_characters_ask_and_retrigger_characters_only_while_shown() {
        let triggers = ["(".to_string(), ",".to_string()];
        let retriggers = [")".to_string()];
        assert!(is_trigger('(', &triggers, &retriggers, false));
        assert!(is_trigger(',', &triggers, &retriggers, true));
        assert!(!is_trigger(')', &triggers, &retriggers, false));
        assert!(is_trigger(')', &triggers, &retriggers, true));
        assert!(!is_trigger('a', &triggers, &retriggers, true));
        assert!(!is_trigger('(', &[], &[], true));
    }

    #[test]
    fn the_arrows_go_around_the_ends() {
        assert_eq!(cycle(0, 3, 1), 1);
        assert_eq!(cycle(2, 3, 1), 0);
        assert_eq!(cycle(0, 3, -1), 2);
        assert_eq!(cycle(1, 3, -1), 0);
        assert_eq!(cycle(0, 1, 1), 0);
        assert_eq!(cycle(0, 0, 1), 0);
    }

    #[test]
    fn the_server_is_told_the_signature_the_user_chose() {
        let help: SignatureHelp = serde_json::from_value(json!({
            "signatures": [{"label": "open(path)"}, {"label": "open(path, mode)"}],
            "activeSignature": 0,
            "activeParameter": 1,
        }))
        .unwrap();
        assert_eq!(shown_help(&help, None), help);
        let chosen = shown_help(&help, Some(1));
        assert_eq!(chosen.active_signature, Some(1));
        assert_eq!(chosen.active_parameter, Some(1));
        assert_eq!(chosen.signatures, help.signatures);
    }
}
