use super::{palette, project};
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode};
use gpui_kit::{Hsla, Rgba, WindowAppearance, px, rgb};
use std::rc::Rc;

fn assert_projection(theme: &Theme, mode: ThemeMode) {
    let p = palette(mode);
    assert_eq!(theme.mode, mode);
    assert_eq!(theme.background, p.content);
    assert_eq!(theme.foreground, p.foreground);
    assert_eq!(theme.muted_foreground, p.secondary_foreground);
    assert_eq!(theme.primary, p.accent);
    assert_eq!(theme.primary_foreground, p.accent_foreground);
    assert_eq!(theme.radius, px(6.));
    assert_eq!(theme.radius_lg, px(10.));
    assert_eq!(
        theme.font_size,
        px(16.),
        "spacing keeps the toolkit rem basis"
    );
    assert_eq!(theme.font_family.as_ref(), ".SystemUIFont");
    assert_eq!(theme.tokens.background.color, theme.background);
    assert_eq!(theme.tokens.background.background, p.content.into());
    assert_eq!(theme.tokens.button_primary.color, p.accent);
    assert_eq!(theme.tokens.popover.color, p.surface);
    let semantic = theme.semantic_tokens();
    assert_eq!(semantic.colors.background, p.content);
    assert_eq!(semantic.colors.surface, p.surface);
    assert_eq!(semantic.colors.ring, p.focus);
    assert_eq!(semantic.radius.md, theme.radius);
    for color in [
        theme.background,
        theme.popover,
        theme.highlight_theme.style.editor_background.unwrap(),
        theme
            .highlight_theme
            .style
            .editor_gutter_background
            .unwrap(),
    ] {
        assert_eq!(color.a, 1., "text-heavy surfaces must stay opaque");
    }
}

fn reload(theme: &mut Theme, mode: ThemeMode) {
    let config: ThemeConfig = serde_json::from_value(serde_json::json!({
        "name": "Simulated system mode",
        "mode": mode,
        "radius": 2,
        "radius_lg": 4,
    }))
    .unwrap();
    theme.apply_config(&Rc::new(config));
    theme.highlight_theme = if mode.is_dark() {
        HighlightTheme::default_dark()
    } else {
        HighlightTheme::default_light()
    };
}

#[test]
fn appearance_dark_projection_before_first_frame() {
    let mut theme = Theme::default();
    reload(&mut theme, ThemeMode::Dark);
    let syntax = theme.highlight_theme.style.syntax.clone();
    project(&mut theme);
    assert_projection(&theme, ThemeMode::Dark);
    assert_eq!(theme.highlight_theme.style.syntax, syntax);
}

#[test]
fn appearance_reapplies_after_each_toolkit_mode_reload() {
    // This exercises the pure projection after public toolkit config reloads.
    // Pinned test-support cannot resolve without changing runtime dependencies;
    // it does not establish observer liveness or native OS event delivery.
    let mut theme = Theme::default();
    for appearance in [
        WindowAppearance::Light,
        WindowAppearance::Dark,
        WindowAppearance::VibrantLight,
        WindowAppearance::VibrantDark,
        WindowAppearance::Light,
        WindowAppearance::Dark,
    ] {
        let mode = ThemeMode::from(appearance);
        reload(&mut theme, mode);
        let syntax = theme.highlight_theme.style.syntax.clone();
        project(&mut theme);
        assert_projection(&theme, mode);
        assert_eq!(theme.highlight_theme.style.syntax, syntax);
        let tokens = theme.tokens;
        project(&mut theme);
        assert_eq!(theme.tokens, tokens, "reapplying is idempotent");
    }
}

fn composite(foreground: Hsla, background: Hsla) -> Hsla {
    Rgba::from(background).blend(foreground.into()).into()
}

fn luminance(color: Hsla) -> f32 {
    let color = Rgba::from(color);
    let linear = |value: f32| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(foreground: Hsla, background: Hsla) -> f32 {
    let foreground = luminance(composite(foreground, background));
    let background = luminance(background);
    (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
}

fn assert_contrast(foreground: Hsla, background: Hsla, minimum: f32, role: &str) {
    let ratio = contrast(foreground, background);
    assert!(
        ratio >= minimum,
        "{role}: {ratio:.2}:1 is below {minimum}:1"
    );
}

#[test]
fn appearance_text_and_focus_contrast() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let p = palette(mode);
        for background in [p.content, p.surface, p.selected, p.hover(), p.pressed()] {
            assert_contrast(p.foreground, background, 4.5, "primary text");
            assert_contrast(p.secondary_foreground, background, 4.5, "secondary text");
            assert_contrast(p.focus.alpha(0.5), background, 3., "toolkit focus band");
        }
        for (foreground, background, role) in [
            (p.selected_foreground, p.selected, "selected navigation"),
            (p.accent_foreground, p.accent, "accent action"),
            (p.accent_foreground, p.accent_hover(), "accent action hover"),
            (
                p.accent_foreground,
                p.accent_pressed(),
                "accent action pressed",
            ),
            (p.error, p.error_surface, "error notice"),
            (p.warning, p.warning_surface, "warning notice"),
            (p.success, p.success_surface, "success notice"),
        ] {
            assert_contrast(foreground, background, 4.5, role);
        }
        for endpoint in [rgb(0x000000).into(), rgb(0xFFFFFF).into()] {
            let chrome = composite(p.chrome, endpoint);
            assert_contrast(p.foreground, chrome, 4.5, "chrome primary text");
            assert_contrast(p.secondary_foreground, chrome, 4.5, "chrome secondary text");
            assert_contrast(p.focus.alpha(0.5), chrome, 3., "chrome toolkit focus band");
        }
    }
}

#[test]
fn appearance_component_state_contrast() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let mut theme = Theme::default();
        reload(&mut theme, mode);
        project(&mut theme);
        for (foreground, backgrounds) in [
            (
                theme.button_foreground,
                [theme.button, theme.button_hover, theme.button_active],
            ),
            (
                theme.button_primary_foreground,
                [
                    theme.button_primary,
                    theme.button_primary_hover,
                    theme.button_primary_active,
                ],
            ),
            (
                theme.button_secondary_foreground,
                [
                    theme.button_secondary,
                    theme.button_secondary_hover,
                    theme.button_secondary_active,
                ],
            ),
            (
                theme.button_danger_foreground,
                [
                    theme.button_danger,
                    theme.button_danger_hover,
                    theme.button_danger_active,
                ],
            ),
            (
                theme.button_warning_foreground,
                [
                    theme.button_warning,
                    theme.button_warning_hover,
                    theme.button_warning_active,
                ],
            ),
            (
                theme.button_success_foreground,
                [
                    theme.button_success,
                    theme.button_success_hover,
                    theme.button_success_active,
                ],
            ),
            (
                theme.button_info_foreground,
                [
                    theme.button_info,
                    theme.button_info_hover,
                    theme.button_info_active,
                ],
            ),
        ] {
            for background in backgrounds {
                assert_contrast(foreground, background, 4.5, "component control state");
            }
        }
        // If outside rings are disabled, primary buttons use an inset 60% line.
        for background in [
            theme.button_primary,
            theme.button_primary_hover,
            theme.button_primary_active,
        ] {
            assert_contrast(
                theme.button_primary_foreground.alpha(0.6),
                background,
                3.,
                "filled action focus line",
            );
        }
    }
}
