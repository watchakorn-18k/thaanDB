//! Desktop app (GPUI). One client of `thaan-core`; see AGENTS.md.

// A GUI app on Windows: no console window next to it. Debug builds keep the console
// for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod broker_console;
mod docview;
mod grid;
mod key_browser;
mod logo;
mod theme;

use gpui_kit::*;
use std::sync::Arc;
use thaan_core::engine::Engine;
use thaan_core::store::Store;

fn main() {
    let store = Store::default_path()
        .and_then(|p| Store::open(p).map_err(|e| eprintln!("store: {e}")).ok())
        .unwrap_or_else(Store::in_memory);
    let engine = Engine::new(store);
    // Passwords persist in the OS keychain; without the feature they last one run.
    #[cfg(feature = "keychain")]
    let engine = engine.with_secrets(Arc::new(thaan_keychain::Keychain));
    let engine = Arc::new(engine);
    #[cfg(feature = "sqlite")]
    engine.register_driver(Arc::new(thaan_driver_sqlite::SqliteDriver));
    #[cfg(feature = "postgres")]
    engine.register_driver(Arc::new(thaan_driver_postgres::PostgresDriver));
    #[cfg(feature = "mysql")]
    engine.register_driver(Arc::new(thaan_driver_mysql::MysqlDriver));
    #[cfg(feature = "mongodb")]
    engine.register_driver(Arc::new(thaan_driver_mongodb::MongoDriver));
    #[cfg(feature = "redis")]
    engine.register_driver(Arc::new(thaan_driver_redis::RedisDriver));
    #[cfg(feature = "mqtt")]
    engine.register_driver(Arc::new(thaan_driver_mqtt::MqttDriver));

    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let settings = engine.settings();
            theme::apply_settings(&settings, None, cx);
            app::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            // No native title bar: the traffic lights sit inside our own header row
            // (`TitleBar` in app.rs), which also drags the window.
            let mut titlebar = gpui_kit::component::TitleBar::title_bar_options();
            titlebar.title = Some("ThaanDB".into()); // Mission Control and the Window menu
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(titlebar),
                window_min_size: Some(size(px(800.), px(500.))),
                ..gpui_kit::component::TitleBar::window_options()
            };
            let engine = engine.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| app::ThaanApp::new(engine, window, cx))
            })
            .expect("failed to open window");
            // Opening a window syncs the system appearance; apply our palette after that.
            cx.defer(move |cx| {
                theme::apply_settings(&settings, None, cx);
                cx.refresh_windows();
            });
            cx.activate(true);
        });
}
