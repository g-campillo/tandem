//! Settings → Appearance and Settings → Fonts: the window opacity and blur sliders, and
//! each pane's font, size and ligatures. Everything applies at once; a slider's value is
//! saved when it is let go, the rest by the next `persist_look`.
use super::fonts::FontPane;
use super::look::{
    BlurBackend, FontChange, blur_label, font_size_label, opacity_label, pane_family, pane_size,
};
use super::panels::note;
use super::*;
use crate::appearance::{palette, panel_body, panel_heading, panel_metadata, panel_warning};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_core::appearance::{
    AppearancePrefs, FONT_SIZE, family_of_label, font_choices, font_label,
};

const APPEARANCE_NOTE: &str = "Opacity fades the whole window, text included, so you can see what is behind it. It stops at 30% so the window cannot disappear. Blur softens what shows through; 0 turns it off. Both apply to every project.";
const BLUR_FALLBACK_NOTE: &str = "This Mac does not let Tandem set the blur strength, so Tandem uses the standard macOS blur: any strength above 0 turns it on.";
const FONTS_NOTE: &str = "Fonts apply to every project. ⌘= and ⌘− change the editor's size. A font that is no longer installed is replaced by Menlo for code and by the system font for text.";

/// The panes with a font list, in the order of `LookControls::families`.
const FAMILY_PANES: [FontPane; 5] = [
    FontPane::Editor,
    FontPane::Terminal,
    FontPane::ChatText,
    FontPane::ChatCode,
    FontPane::Explorer,
];
/// The panes with a size slider, in the order of `LookControls::sizes`. The chat has one
/// size, which `ChatText` stands for.
const SIZE_PANES: [FontPane; 4] = [
    FontPane::Editor,
    FontPane::Terminal,
    FontPane::ChatText,
    FontPane::Explorer,
];

/// A searchable list of the installed font families, by the names Settings shows.
type FontList = SelectState<SearchableVec<SharedString>>;

/// The controls of Settings → Appearance and Settings → Fonts.
pub(super) struct LookControls {
    opacity: Entity<SliderState>,
    blur: Entity<SliderState>,
    /// The font lists, in the order of `FAMILY_PANES`.
    families: [Entity<FontList>; 5],
    /// The font size sliders, in the order of `SIZE_PANES`.
    sizes: [Entity<SliderState>; 4],
    _subscriptions: Vec<Subscription>,
}

impl LookControls {
    /// Controls set to `prefs`, with font lists of the `installed` families. Each change
    /// applies to the window at once; a slider saves the appearance when released.
    pub fn new(
        prefs: &AppearancePrefs,
        installed: &[String],
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(30.)
                .max(100.)
                .step(1.)
                .default_value(f32::from(prefs.opacity))
        });
        let blur = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(f32::from(prefs.blur))
        });
        let mut subscriptions =
            vec![
                cx.subscribe_in(
                    &opacity,
                    window,
                    |this, _, event: &SliderEvent, window, cx| match event {
                        SliderEvent::Change(SliderValue::Single(value)) => {
                            this.set_opacity(*value, window, cx)
                        }
                        SliderEvent::Release(_) => this.persist_look(cx),
                        SliderEvent::Change(SliderValue::Range(..)) => {}
                    },
                ),
                cx.subscribe_in(&blur, window, |this, _, event: &SliderEvent, window, cx| {
                    match event {
                        SliderEvent::Change(SliderValue::Single(value)) => {
                            this.set_blur(*value, window, cx)
                        }
                        SliderEvent::Release(_) => this.persist_look(cx),
                        SliderEvent::Change(SliderValue::Range(..)) => {}
                    }
                }),
            ];

        let choices: Vec<SharedString> = font_choices(installed)
            .iter()
            .map(|family| SharedString::from(font_label(family).to_owned()))
            .collect();
        let families = FAMILY_PANES.map(|pane| {
            // A saved family that is not installed is not in the list: nothing is selected.
            let saved = font_label(pane_family(prefs, pane));
            let row = choices.iter().position(|label| label.as_ref() == saved);
            let items = SearchableVec::new(choices.clone());
            cx.new(|cx| {
                SelectState::new(items, row.map(IndexPath::new), window, cx).searchable(true)
            })
        });
        for (pane, state) in FAMILY_PANES.into_iter().zip(&families) {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event {
                        let family = family_of_label(label).to_owned();
                        this.change_font(FontChange::Family(pane, family), window, cx);
                    }
                },
            ));
        }

        let sizes = SIZE_PANES.map(|pane| {
            let size = pane_size(prefs, pane);
            cx.new(|_| {
                SliderState::new()
                    .min(FONT_SIZE.0)
                    .max(FONT_SIZE.1)
                    .step(1.)
                    .default_value(size)
            })
        });
        for (pane, state) in SIZE_PANES.into_iter().zip(&sizes) {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, _, event: &SliderEvent, window, cx| match event {
                    SliderEvent::Change(SliderValue::Single(value)) => {
                        this.change_font(FontChange::Size(pane, *value), window, cx)
                    }
                    SliderEvent::Release(_) => this.persist_look(cx),
                    SliderEvent::Change(SliderValue::Range(..)) => {}
                },
            ));
        }

        Self {
            opacity,
            blur,
            families,
            sizes,
            _subscriptions: subscriptions,
        }
    }

    /// Moves each size slider to the size in `prefs`, so that ⌘= and ⌘− move the editor's.
    /// A slider that already shows its size is left alone, so a drag never fights itself.
    pub fn sync(&self, prefs: &AppearancePrefs, window: &mut Window, cx: &mut App) {
        for (pane, state) in SIZE_PANES.into_iter().zip(&self.sizes) {
            let size = pane_size(prefs, pane);
            if state.read(cx).value() != SliderValue::Single(size) {
                state.update(cx, |state, cx| state.set_value(size, window, cx));
            }
        }
    }
}

impl Workspace {
    /// The Appearance section of Settings: window opacity and background blur.
    pub(super) fn appearance_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let row = |id: &'static str, label: &'static str, slider: Slider, value: String| {
            div()
                .id(id)
                .h_flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .child(div().w(px(150.)).flex_none().child(label))
                .child(div().flex_1().min_w_0().child(slider))
                .child(
                    div()
                        .w(px(44.))
                        .flex_none()
                        .text_right()
                        .text_color(p.secondary_foreground)
                        .child(value),
                )
        };
        panel_body(cx)
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(panel_heading(note("Appearance")))
            .child(row(
                "settings-opacity",
                "Window opacity",
                Slider::new(&self.look_controls.opacity).accessibility_label("Window opacity"),
                opacity_label(self.look.prefs.opacity),
            ))
            .child(row(
                "settings-blur",
                "Background blur",
                Slider::new(&self.look_controls.blur)
                    .accessibility_label("Background blur strength"),
                blur_label(self.look.prefs.blur),
            ))
            .when(self.look.blur == BlurBackend::BuiltIn, |d| {
                d.child(panel_warning(note(BLUR_FALLBACK_NOTE), cx))
            })
            .child(panel_metadata(note(APPEARANCE_NOTE), cx))
            .into_any_element()
    }

    /// The Fonts section of Settings: the font, size and ligatures of the editor, the
    /// terminal, the chat and the file explorer, and which saved fonts are not installed.
    pub(super) fn font_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx.theme().mode);
        let prefs = &self.look.prefs;
        let controls = &self.look_controls;
        let weak = cx.entity().downgrade();
        let row = |id: &'static str, label: &'static str, control: AnyElement| {
            div()
                .id(id)
                .h_flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .child(div().w(px(150.)).flex_none().child(label))
                .child(div().flex_1().min_w_0().child(control))
        };
        let font_row = |id: &'static str, label: &'static str, list: usize, name: &'static str| {
            let pane = FAMILY_PANES[list];
            let missing = self
                .look
                .fonts
                .substitutions
                .iter()
                .find(|substitution| substitution.pane == pane)
                .map(|substitution| {
                    format!("{} (not installed)", font_label(&substitution.missing))
                });
            let select = Select::new(&controls.families[list])
                .small()
                .search_placeholder("Search fonts")
                .accessibility_label(name)
                .when_some(missing, |select, missing| select.placeholder(missing));
            row(id, label, select.into_any_element())
        };
        let size_row = |id: &'static str, slider: usize, name: &'static str| {
            let size = pane_size(prefs, SIZE_PANES[slider]);
            row(
                id,
                "Size",
                Slider::new(&controls.sizes[slider])
                    .accessibility_label(name)
                    .into_any_element(),
            )
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .text_right()
                    .text_color(p.secondary_foreground)
                    .child(font_size_label(size)),
            )
        };
        let ligatures = |id: &'static str, label: &'static str, pane: FontPane, checked: bool| {
            let weak = weak.clone();
            Switch::new(id)
                .label(label)
                .checked(checked)
                .on_click(move |checked, window, cx| {
                    let checked = *checked;
                    let _ = weak.update(cx, |view, cx| {
                        view.change_font(FontChange::Ligatures(pane, checked), window, cx)
                    });
                })
        };
        let block = |title: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .min_w_0()
                .child(div().text_size(px(13.)).font_semibold().child(title))
        };
        let fonts = &prefs.fonts;
        panel_body(cx)
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(panel_heading(note("Fonts")))
            .child(
                block("Editor")
                    .child(font_row("settings-font-editor", "Font", 0, "Editor font"))
                    .child(size_row("settings-font-editor-size", 0, "Editor font size"))
                    .child(ligatures(
                        "font-editor-ligatures",
                        "Ligatures",
                        FontPane::Editor,
                        fonts.editor.ligatures,
                    )),
            )
            .child(
                block("Terminal")
                    .child(font_row(
                        "settings-font-terminal",
                        "Font",
                        1,
                        "Terminal font",
                    ))
                    .child(size_row(
                        "settings-font-terminal-size",
                        1,
                        "Terminal font size",
                    ))
                    .child(ligatures(
                        "font-terminal-ligatures",
                        "Ligatures",
                        FontPane::Terminal,
                        fonts.terminal.ligatures,
                    )),
            )
            .child(
                block("Chat")
                    .child(font_row(
                        "settings-font-chat-text",
                        "Text font",
                        2,
                        "Chat text font",
                    ))
                    .child(font_row(
                        "settings-font-chat-code",
                        "Code font",
                        3,
                        "Chat code font",
                    ))
                    .child(size_row("settings-font-chat-size", 2, "Chat font size"))
                    .child(ligatures(
                        "font-chat-ligatures",
                        "Ligatures in code",
                        FontPane::ChatCode,
                        fonts.chat.code_ligatures,
                    )),
            )
            .child(
                block("File explorer")
                    .child(font_row(
                        "settings-font-explorer",
                        "Font",
                        4,
                        "File explorer font",
                    ))
                    .child(size_row(
                        "settings-font-explorer-size",
                        3,
                        "File explorer font size",
                    )),
            )
            .children(self.look.fonts.substitutions.iter().enumerate().map(
                |(index, substitution)| {
                    let text = SharedString::from(substitution.note());
                    panel_warning(
                        div()
                            .id(("font-substitution", index))
                            .role(Role::Label)
                            .aria_label(text.clone())
                            .child(text),
                        cx,
                    )
                },
            ))
            .child(panel_metadata(note(FONTS_NOTE), cx))
            .into_any_element()
    }
}
