//! The Settings window: the Settings view in a window of its own, beside the editor. The
//! window draws the workspace's view, so a change made in it reaches the editor at once
//! and a change made elsewhere shows in it. Its traffic lights close it, and so does ⌘W
//! (handled with the other app-wide shortcuts in `install_actions`).
use super::*;
use crate::appearance::palette;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _};

/// The root of the Settings window.
pub(super) struct SettingsWindow {
    workspace: WeakEntity<Workspace>,
    _workspace_changed: Subscription,
}

/// Opens the Settings window of `workspace`.
pub(super) fn open(workspace: Entity<Workspace>, cx: &mut App) -> Result<AnyWindowHandle> {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(760.), px(720.)),
            cx,
        ))),
        window_min_size: Some(size(px(480.), px(360.))),
        ..Default::default()
    };
    let (handle, _) = gpui_kit::open_window(options, cx, move |_, cx| {
        cx.new(|cx| SettingsWindow::new(workspace, cx))
    })?;
    Ok(handle)
}

impl SettingsWindow {
    fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let changed = cx.observe(&workspace, |_, _, cx| cx.notify());
        let weak = workspace.downgrade();
        cx.on_release(move |_, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.settings.window = None;
                cx.notify();
            });
        })
        .detach();
        Self {
            workspace: workspace.downgrade(),
            _workspace_changed: changed,
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx.theme().mode);
        let content = self
            .workspace
            .update(cx, |view, cx| view.settings_view(cx))
            .ok();
        // A column, so the view below is held to the window's height and scrolls.
        div()
            .id("settings-window")
            .v_flex()
            .size_full()
            .bg(p.surface)
            .text_color(p.foreground)
            .text_size(px(13.))
            .children(content)
    }
}
