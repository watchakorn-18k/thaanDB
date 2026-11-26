//! ThaanDB in the browser: a demo of the desktop app for GitHub Pages.
//!
//! It runs the same engine (`thaan-core`), the same SQLite driver and the same result grid
//! and theme as the desktop app, on a small sample shop database kept in memory. Servers
//! (PostgreSQL, MySQL, MongoDB, Redis) need TCP, which a page cannot open, so they stay in
//! the desktop app; MQTT works over WebSocket (`mqtt_ws.rs`).
//!
//! Built for `wasm32-unknown-unknown` with nightly Rust (GPUI's web scheduler needs it) and
//! `wasm-bindgen`; see `scripts/build-web.sh`. GPUI runs single threaded, so the page needs
//! no cross-origin isolation headers (GitHub Pages sets none).
#![cfg(target_family = "wasm")]

/// Actions the desktop grid's menus dispatch. In the demo they have no handlers.
mod app {
    gpui_kit::actions!(thaan, [CopySelection, PasteCell, CloneRow, DeleteRow]);
}

/// The one helper the grid takes from the desktop's document view.
mod docview {
    use gpui_kit::component::Theme;
    use gpui_kit::*;
    use thaan_core::doc::DocKind;

    pub fn kind_color(kind: DocKind, theme: &Theme) -> Hsla {
        match kind {
            DocKind::String => theme.foreground,
            DocKind::Number => theme.cyan,
            DocKind::Bool => theme.yellow,
            DocKind::Date => theme.blue,
            DocKind::Id => theme.muted_foreground,
            DocKind::Object | DocKind::Array => theme.primary,
            DocKind::Null => theme.muted_foreground.opacity(0.7),
            DocKind::Other => theme.magenta,
        }
    }
}

// The desktop app's grid and theme, unchanged.
#[allow(dead_code)]
#[path = "../../thaan-app/src/grid.rs"]
mod grid;
#[path = "../../thaan-app/src/logo.rs"]
mod logo;
#[allow(dead_code)]
#[path = "../../thaan-app/src/theme.rs"]
mod theme;

mod demo;
mod mqtt_ws;
mod view;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::borrow::Cow;
use wasm_bindgen::prelude::*;

/// The browser has no system fonts GPUI can read: these are embedded (fonts/SOURCES.md).
const FONTS: [&[u8]; 5] = [
    include_bytes!("../fonts/Inter-Regular.ttf"),
    include_bytes!("../fonts/Inter-Medium.ttf"),
    include_bytes!("../fonts/Inter-SemiBold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Medium.ttf"),
];

fn use_fonts(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    theme.font_family = "Inter".into();
    theme.mono_font_family = "JetBrains Mono".into();
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    pub(crate) fn console_error(s: &str);
}

/// The page's folder as an absolute URL without the trailing slash
/// (`https://user.github.io/thaanDB`): the asset loader needs absolute URLs.
fn page_dir() -> String {
    let base = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.base_uri().ok().flatten())
        .unwrap_or_default();
    match base.rfind('/') {
        Some(i) => base[..i].to_string(),
        None => base,
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    gpui_kit::platform::web_init();
    gpui_kit::platform::single_threaded_web()
        // Icons are fetched from assets/icons next to the page (copied by build-web.sh).
        .with_assets(gpui_kit::assets::Assets::new(page_dir()))
        .run(|cx| {
            gpui_kit::init(cx);
            let fonts = FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
            if let Err(e) = cx.text_system().add_fonts(fonts) {
                console_error(&format!("ThaanDB: could not load fonts: {e}"));
            }
            // The desktop theme (teal accent, dark), then our fonts: changing the theme
            // resets them.
            theme::apply(ThemeMode::Dark, None, cx);
            use_fonts(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| view::Demo::new(window, cx))
            })
            .ok();
        });
}
