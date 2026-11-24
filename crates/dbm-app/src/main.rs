//! Desktop app (GPUI). One client of `dbm-core`; see AGENTS.md.

mod app;
mod grid;
mod theme;

use dbm_core::engine::Engine;
use dbm_core::store::Store;
use gpui_kit::component::ThemeMode;
use gpui_kit::*;
use std::sync::Arc;

fn main() {
    let store = Store::default_path()
        .and_then(|p| Store::open(p).map_err(|e| eprintln!("store: {e}")).ok())
        .unwrap_or_else(Store::in_memory);
    let engine = Arc::new(Engine::new(store));
    engine.register_driver(Arc::new(dbm_driver_sqlite::SqliteDriver));

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::apply(ThemeMode::Dark, None, cx);
            app::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("DB Manager".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(800.), px(500.))),
                ..Default::default()
            };
            let engine = engine.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| app::DbmApp::new(engine, window, cx))
            })
            .expect("failed to open window");
            // Opening a window syncs the system appearance; apply our palette after that.
            cx.defer(|cx| {
                theme::apply(ThemeMode::Dark, None, cx);
                cx.refresh_windows();
            });
            cx.activate(true);
        });
}
