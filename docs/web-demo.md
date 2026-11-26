# Browser demo

`crates/thaan-web` runs ThaanDB in a web page, for GitHub Pages. It is a demo of the
desktop app, not a replacement for it.

## What it has

- The same engine (`thaan-core`), SQLite driver, result grid (`thaan-app/src/grid.rs`) and
  theme (`thaan-app/src/theme.rs`) as the desktop app, on a sample shop database
  (`customers`, `products`, `orders`) built in memory at start-up (`src/demo.rs`). Every
  visitor gets their own copy; nothing is saved.
- A SQL editor that runs any statement on that database.
- An MQTT console over WebSocket (`src/mqtt_ws.rs`): it reuses the desktop driver's packet
  code (`thaan_driver_mqtt::packet`) with the browser's WebSocket as transport, so it works
  with brokers that accept MQTT over WebSocket (`wss://`, subprotocol `mqtt`).

## What it cannot have

- PostgreSQL, MySQL, MongoDB, Redis and plain MQTT: they need TCP, which a page cannot open.
- The MCP server, the CLI, the OS keychain, files on disk.

## How it is built

`scripts/build-web.sh` (or the `Web demo` workflow) builds it into `target/web`:

- **Nightly Rust.** GPUI's web backend pulls in `wasm_thread`, which uses a nightly
  feature. GPUI runs single threaded (`gpui_kit::platform::single_threaded_web()`), so the
  page needs no `SharedArrayBuffer` and no cross-origin isolation headers, which GitHub
  Pages cannot send. Rendering is WebGPU with a WebGL2 fallback.
- **SQLite.** `rusqlite` is built without `bundled`; `sqlite-wasm-rs` compiles SQLite for
  `wasm32-unknown-unknown` (it needs a clang with the WebAssembly backend: Homebrew LLVM on
  macOS, `clang` + `llvm` on Linux) and provides the `sqlite3_*` symbols. The script gives
  `libsqlite3-sys` an empty `libsqlite3.a` so its link request is satisfied.
- **Clock.** `thaan-core` uses `web-time` on wasm: `std::time` panics in the browser.
- **Fonts.** A page has no system fonts GPUI can read, so Inter and JetBrains Mono are
  embedded (`crates/thaan-web/fonts`, SIL OFL 1.1, see `SOURCES.md`).
- **Icons.** On the web the kit fetches icons over HTTP from `assets/icons` next to the
  page; the script copies the lucide set there.
- **Logo.** The top bar draws the desktop title bar's mark (`thaan-app/src/logo.rs`, built
  from cells in the theme's colours); the page's favicons come from `docs/brand/logo`, with
  paths relative to the page.
- `wasm-bindgen-cli` must match the `wasm-bindgen` version in `Cargo.lock`.

Try it locally:

```bash
scripts/build-web.sh
python3 -m http.server 8791 --directory target/web
```

then open <http://localhost:8791>. On native targets the crate is empty, so workspace
builds, tests and clippy are unaffected.

## Publishing

One-time: in the repository's **Settings > Pages > Build and deployment**, set **Source**
to **GitHub Actions**. After that, every push to `main` that touches the demo's code (or
**Run workflow** on the `Web demo` workflow) builds and deploys it to
`https://<owner>.github.io/<repo>/`.

## Known limits

- The first load downloads about 20 MB of WebAssembly (served compressed).
- Typing in text fields uses GPUI's own input handling on a canvas; browser autofill and
  some keyboard shortcuts behave differently from a normal web form.
- The default broker in the MQTT console is a public test broker: messages sent there are
  visible to anyone subscribed to the same topics.
