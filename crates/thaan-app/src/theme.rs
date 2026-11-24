//! Our own palette on top of the component theme (see docs/spec/ui.md).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

/// Teal accent, chosen for this project.
fn accent(dark: bool) -> Hsla {
    if dark {
        rgb(0x14b8a6).into()
    } else {
        rgb(0x0f766e).into()
    }
}

pub fn apply(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
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
        // White on the bright dark-mode teal is ~2.5:1, so dark mode uses near-black ink
        // (~8:1). The light-mode teal is dark enough for white (~5.4:1).
        let ink: Hsla = if dark {
            rgb(0x06201d).into()
        } else {
            rgb(0xffffff).into()
        };
        t.button_primary_foreground = ink;
        t.primary_foreground = ink;
    });
}

pub fn toggle(window: &mut Window, cx: &mut App) {
    let mode = if cx.theme_is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    apply(mode, Some(window), cx);
}

trait IsDark {
    fn theme_is_dark(&self) -> bool;
}

impl IsDark for App {
    fn theme_is_dark(&self) -> bool {
        Theme::global(self).is_dark()
    }
}
