//! Settings → Appearance: the window opacity and blur sliders. Both apply while dragging
//! and are saved when the slider is let go.
use super::look::{BlurBackend, blur_label, opacity_label};
use super::panels::note;
use super::*;
use crate::appearance::{palette, panel_body, panel_heading, panel_metadata, panel_warning};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder;
use tandem_core::appearance::AppearancePrefs;

const APPEARANCE_NOTE: &str = "Opacity fades the whole window, text included, so you can see what is behind it. It stops at 30% so the window cannot disappear. Blur softens what shows through; 0 turns it off. Both apply to every project.";
const BLUR_FALLBACK_NOTE: &str = "This Mac does not let Tandem set the blur strength, so Tandem uses the standard macOS blur: any strength above 0 turns it on.";

/// The sliders of Settings → Appearance.
pub(super) struct LookControls {
    opacity: Entity<SliderState>,
    blur: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl LookControls {
    /// Sliders set to `prefs` that apply each change to the window while dragging and save
    /// the appearance when released.
    pub fn new(prefs: &AppearancePrefs, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
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
        let subscriptions =
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
        Self {
            opacity,
            blur,
            _subscriptions: subscriptions,
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
}
