//! Our own palette on top of the component theme (see docs/spec/ui.md).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use thaan_core::store::{Settings, ThemePreference};

/// Teal accent, chosen for this project.
fn accent(dark: bool) -> Hsla {
    if dark {
        rgb(0x14b8a6).into()
    } else {
        rgb(0x0f766e).into()
    }
}

pub fn apply(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    // Changing the mode reloads the theme; keep the text size the user picked.
    let font_size = cx
        .has_global::<Theme>()
        .then(|| Theme::global(cx).font_size);
    Theme::change(mode, window, cx);
    let dark = mode.is_dark();
    let a = accent(dark);
    // `Theme::update` reconciles the legacy colour fields into the design tokens.
    Theme::update(cx, |t| {
        t.primary = a;
        t.primary_hover = a.opacity(0.9);
        t.primary_active = a.opacity(0.8);
        t.button_primary = a;
        t.button_primary_hover = a.opacity(0.9);
        t.button_primary_active = a.opacity(0.8);
        t.ring = a;
        t.caret = a;
        t.tab_active_foreground = a;
        // The selected grid cell: no fill and no square kit frame. The grid draws its own
        // rounded accent frame and corner handle over the whole cell (grid.rs).
        t.table_active = a.opacity(0.0);
        t.table_active_border = a.opacity(0.0);
        // White on the bright dark-mode teal is ~2.5:1, so dark mode uses near-black ink
        // (~8:1). The light-mode teal is dark enough for white (~5.4:1).
        let ink: Hsla = if dark {
            rgb(0x06201d).into()
        } else {
            rgb(0xffffff).into()
        };
        t.button_primary_foreground = ink;
        t.primary_foreground = ink;
        if let Some(f) = font_size {
            t.font_size = f;
        }
    });
}

/// The mode a theme preference stands for right now (System follows the window, or the
/// OS before a window exists).
pub fn mode_for(pref: ThemePreference, window: Option<&Window>, cx: &App) -> ThemeMode {
    match pref {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => window
            .map(|w| w.appearance())
            .unwrap_or_else(|| cx.window_appearance())
            .into(),
    }
}

/// Applies the stored appearance settings: theme and text size.
pub fn apply_settings(settings: &Settings, window: Option<&mut Window>, cx: &mut App) {
    let mode = mode_for(settings.theme, window.as_deref(), cx);
    apply(mode, window, cx);
    let size = px(settings.font_size as f32);
    Theme::update(cx, |t| t.font_size = size);
}

/// Whether the theme in use is dark.
pub fn is_dark(cx: &App) -> bool {
    Theme::global(cx).is_dark()
}
