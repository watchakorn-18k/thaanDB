# CLAUDE.md

@AGENTS.md

## Notes for Claude Code

- `AGENTS.md` (imported above) is the source of truth for goals, principles, layout and
  commands. Keep both files in sync: project rules go in `AGENTS.md`, Claude-specific notes here.
- **Clean-room is a hard rule.** Do not use the REA / craft-anything / decompiler / Frida
  tools, `npx asar extract`, or plain `cat`/`grep` on another database client's installed
  app (for example anything under `/Applications/*.app` that is a DB client) or its source
  repository. If a task asks for that, stop and point to `docs/clean-room.md`. Using such an
  app as a user (screenshots of its UI) to write a behaviour spec in `docs/spec/` is fine.
- **Speed and memory come first.** This is the working principle of the whole repo; see
  [Speed and memory](#speed-and-memory) below. It outranks features and looks.
- Build, check and test with `cargo`, using the commands in `AGENTS.md` (`cargo build`,
  `cargo test`, `cargo clippy`, `cargo fmt`). Do not use `blaze`.
- Before running the GUI, build headless crates first: `cargo test --workspace --exclude thaan-app`.
  The first `cargo build -p thaan-app` compiles GPUI and takes several minutes; run it in
  the background.
- GPUI APIs change between versions. Check the pinned version in `crates/thaan-app/Cargo.toml`
  and read examples from the crate source in `~/.cargo/registry/src/*/gpui-component-*`
  rather than relying on memory.
- **UI changes start with `/unslop`.** Before designing, changing or fixing anything in the
  GUI (`crates/thaan-app`: layout, spacing, colours, theme tokens, icons, copy, motion),
  invoke the `unslop` skill first and follow it, then make the change. This applies
  every time, including small tweaks. Keep `docs/spec/ui.md` in sync with what it decides.
- When you finish a feature, update its status in `docs/spec/features.md`.
- Seeing the GUI on macOS: the screenshot tools only find bundled apps. Run
  `scripts/bundle-macos.sh` (add `--release` for a release build): it builds `thaan-app` with
  cargo and writes `target/ThaanDB.app` with bundle id `dev.thaandb.app` and the app icon from
  `crates/thaan-app/assets/macos/AppIcon.icns`. It removes the old binary before copying
  (overwriting it in place gets the process killed).
- Do not `codesign` dev builds. On macOS, saved passwords go through Apple's
  `/usr/bin/security` tool (`crates/thaan-keychain/src/lib.rs`, shared with `thaan mcp`), so the Keychain never
  prompts, rebuild or not; signing is not needed for that (a self-signed identity did not help:
  the Keychain trusts per Team ID).
- Windows: `crates/thaan-app/build.rs` links `assets/windows/app.res` (the app icon as
  resource 1, which GPUI loads for the window and taskbar, plus version info) into MSVC
  builds. Regenerate it and `AppIcon.ico` with `scripts/make-windows-icon.sh` when the logo
  or the workspace version changes. Release builds use the Windows GUI subsystem (no console
  window); debug builds keep the console for logs.
- Releases: only pushing a `v*` tag runs `.github/workflows/release.yml` (Windows x64 zip and
  a universal macOS app on a GitHub release); a manual run on main refreshes the Cargo
  cache tags restore from. See `docs/releasing.md`.
- Cross-compiling for Windows from macOS (`cargo xwin build --release -p thaan-app --target
  x86_64-pc-windows-msvc`) fails in `gpui-pre-windows`: its release build needs `fxc.exe`,
  which only exists on Windows. Build Windows releases on Windows (or CI). A test build from
  a Mac needs a local patch of that crate that compiles the shaders at startup; never commit it.
- The app icon and the title-bar mark come from `docs/brand` (see its README). Regenerate
  `AppIcon.icns` from `docs/brand/logo/thaandb-macos-icon*.svg` with `iconutil` if the logo changes.
- Use `THAAN_HOME=target/thaan-home` for manual testing so the user's real store is untouched.
- `scripts/dev-watch.sh` rebuilds and restarts the app on every save (cargo-watch; `--bundle`
  relaunches `target/ThaanDB.app` for the screenshot tools). It is a restart, not hot reload:
  GPUI cannot swap code into a running app, so tabs and connections are lost each time.
- The dev profile keeps only line tables for our crates and no debug info for dependencies,
  which roughly halves a one-line rebuild of `thaan-app` (29-39 s to 13-23 s measured). For a
  debugger session with variables, build with `CARGO_PROFILE_DEV_DEBUG=true`.
- `pkill -f thaan-app` also matches the shell running the command (its text contains the
  name) and kills it. Use `pkill -x thaan-app`.

## Speed and memory

**The rule of this repo: fast first, light on the machine, as little RAM as possible.**
Everything must open the moment it is asked for and never freeze. Squeeze out all the
performance you can. When waiting cannot be avoided, the user must see that something is
happening. This rule outranks features and looks. Apply it to every change, and treat
breaking it as a bug. (AGENTS.md principle 9 and its Performance section hold the same
rules for all contributors.)

### Instant, never frozen

- Nothing slow runs on the UI thread: no database, network or disk work. Connect, query,
  `list_objects`, `describe_table`, `ddl`, `fetch_page`, export, import and saving the store
  all go through `cx.background_spawn`, and the view updates when they return.
- `render` does no I/O and calls no `Engine` method. Keep the data the view needs (history,
  saved queries, schema) in the view and refresh it when it changes.
- Show what is already known straight away, then fill in the rest. Opening a table shows
  the tab and its header at once and loads rows behind it.
- Long work can be cancelled, and a late answer never overwrites newer state (drop it if
  its tab or request is gone).

### When waiting is unavoidable, say so

- Under ~100 ms: show the result directly, with no indicator (a spinner that flashes reads
  as lag).
- Over ~100 ms: show a loading state at once, right where the content will appear, and say
  in words what it is waiting for:
  - Connecting: `Connecting to shop-prod…` on the button and in the status bar.
  - Running a query: `Running…` on Run, plus elapsed time in the status bar.
  - Loading rows: a skeleton of grid rows (`Loading 50 rows…`), keeping the previous
    page visible while the next one loads.
  - Schema: skeleton rows in the tree while objects load.
  - Export / import: a progress count (`Exporting… 12,400 rows`) with Cancel.
- Use gpui-kit's own pieces where they exist (spinner, skeleton, progress), styled per
  `docs/spec/ui.md`: subtle, one easing curve, no looping motion on things that are not
  waiting.

### Use as little RAM as possible

- Never load a whole table or an unbounded result. Tables are paged (50 rows by default).
  Query results are capped, with a clear "first N rows" note and a way to load more.
- Stream exports and imports to and from disk row by row. Never build the whole file as
  one `String` or `Vec`.
- Keep one copy of each result. Share it (`Arc`) between the tab and the grid instead of
  cloning a `ResultSet`. Free a tab's data when the tab closes.
- Grids and long lists are virtualized. Long cell values are shortened for display, and
  the full value loads only when asked for.
- Load schema lazily (on expand) and cache it per session. Cap history and other stores.
- Startup opens the window first and loads the rest after. No network at startup.
- Add dependencies only when they pay for themselves; prefer the lighter crate. Heavy
  parts stay behind Cargo features. Release builds use the tuned `[profile.release]`.

### Check before you finish

- [ ] No new engine, network or disk call on the UI thread, and none in `render`
- [ ] Nothing loads unbounded data or clones a whole result
- [ ] Every wait over ~100 ms has a loading state in words (spinner, skeleton or progress)
- [ ] For speed or memory claims, measure a release build
  (`cargo build --release -p thaan-app`; RAM via Activity Monitor or `ps -o rss= -p <pid>`)
  and put the numbers in the commit message

### Known gaps in the app today (fix these first)

1. (Fixed) `describe_table` and `ddl` now run in the background: the table tab, the
   sidebar's Open structure and the Structure view's DDL tab (`structure.rs`).
2. Export to file reads the whole table and writes it on the UI thread
   (`export_table_to_file` in `app.rs`; `Engine::export_table` returns one `String`).
3. The History / Saved queries panels call `engine.history(200)` and
   `engine.saved_queries()` on every render (the home screen now reads a cache).
4. Query results have no row cap; `select *` on a big table loads all of it.
5. Each result is held twice: in the tab (`QueryTab.results`, `TableTab.page`) and in the
   grid (`ResultGrid.rs`).
6. No loading state while a table page loads (the grid is empty), while the schema
   refreshes, or while exporting.

## GUI toolkit: gpui-kit

The desktop app (`crates/thaan-app`) is built on **gpui-kit 0.11** only:
`gpui-kit = { version = "0.11.0", features = ["tree-sitter-sql", "tree-sitter-javascript"] }`.
It bundles GPUI (`gpui_kit::*`), the base layer (`gpui_kit::base`), the styled component
library (`gpui_kit::component`, the `gpui-component` crate) and the icon assets
(`gpui_kit::assets`). Do not add `gpui`, `gpui-component` or another UI crate directly.

**Rule: use a kit component before drawing your own.** Build a custom element out of
`div()` / `h_flex()` / `v_flex()` only when the kit has nothing that fits (the home-screen
cards and the grid gutter are examples). When a hand-built piece turns out to have a kit
equivalent, switch to the kit one.

### Setup (already in `main.rs`)

- `gpui_kit::application().with_assets(gpui_kit::assets::AllAssets)`: registers the full
  icon catalog (about 1,830 SVGs, roughly +1 MiB in the binary) so any lucide name works.
- `gpui_kit::init(cx)` once, before anything else.
- Window options start from `TitleBar::window_options()` with `TitleBar::title_bar_options()`
  (transparent title bar, traffic lights inside our header). The header row is a
  `TitleBar` in `app.rs`; put title-bar items there, never a second bar below it.
- `gpui_kit::open_window(options, cx, |window, cx| cx.new(...))`: wraps our view in the
  kit's `Root`. The dialog layer (`window.open_dialog`) only works under that `Root`, so
  never open windows with plain `cx.open_window`.
- Theme: `crates/thaan-app/src/theme.rs` calls `Theme::change(mode)` and then
  `Theme::update(cx, |t| …)` to set our teal accent and button ink. Change colours only
  through `Theme::update` (it keeps colours and tokens in step). Components read colours
  with `cx.theme()` (`use gpui_kit::component::ActiveTheme as _`).

### Components in use and where

| Need | Kit component (`gpui_kit::component::…`) | Used in |
| --- | --- | --- |
| Layout | `h_flex`, `v_flex`, `h_resizable` / `v_resizable` + `resizable_panel` | everywhere; main split and editor/results split in `app.rs` |
| Buttons | `button::{Button, ButtonVariants}` (`.primary()`, `.ghost()`, `.small()`, `.xsmall()`, `.icon()`, `.tooltip()`, `.selected()`) | toolbars, rail, tabs, dialogs |
| Text fields | `input::{Input, InputState, InputEvent}` (`.prefix(icon)`, `.masked(true)` for passwords) | filters, palette search, connection form, page box |
| SQL / JS editor | `input::{Editor, EditorState}` with `.language("sql")` | query tabs, Mongo shell |
| Dropdown pickers | `select::{Select, SelectState, SelectEvent}` with `Vec<SharedString>` items, `IndexPath` for the selection | filter builder (column, operator) |
| Checkbox | `checkbox::Checkbox` | read-only option in the connection form |
| Data grid | `table::{DataTable, TableState, TableDelegate, Column, ColumnSort}` | `grid.rs` (`ResultGrid` delegate: row gutter, typed headers, server sort) |
| Dialog | `WindowExt::open_dialog` / `close_dialog` building a `Dialog` (`.title()`, `.child()`, `.footer()`, `.w()`, `.on_close()`) | New connection dialog in `home.rs` |
| Menus | `menu::{DropdownMenu, PopupMenuItem}`: `Button::dropdown_menu_with_anchor(Anchor::BottomRight, …)` | Export menu in the status bar |
| Shortcuts | `kbd::Kbd` from a GPUI keystroke string (`kbd("cmd-k")` helper in `app.rs`) | title bar, palette rows and legend, home shortcuts |
| Icons | `Icon` via the `icon(name)` helper (below) | everywhere |

Notes from using them:
- `Select` stretches to fill its parent and ignores `.w()`. Wrap it in a fixed-width
  `div().w(px(…)).flex_none()`.
- The `Dialog` builder runs again on every frame, so build it from view state
  (`this.update(cx, |app, cx| app.parts(cx))`) instead of capturing values once.
- Read the component's source in `~/.cargo/registry/src/*/gpui-component-0.11.0/src/` before
  using a new one. Its API differs from older gpui-component versions you may remember.

### Icons

- **Database logos:** each driver shows its own database's logo from Simple Icons (CC0),
  embedded by `crates/thaan-app/src/assets.rs` and drawn with `db_icon(driver_id)` in one
  colour like any other icon. Sources and the trademark note are in
  `crates/thaan-app/assets/db-icons/SOURCES.md`; add new logos the same way. Drivers
  without a logo fall back to lucide `database`. Never take logos from another database client.
- **Set (everything else):** lucide only (version 1.43, ISC licence in `gpui-kit-assets-0.11.0/LICENSE-LUCIDE`),
  shipped by `gpui-kit-assets`. Do not add other icon sets, emoji or other products' logos.
  The exceptions are the ThaanDB logo (`logo.rs`, from `docs/brand`) and the database logos above.
- **How:** `icon("name")` in `app.rs` → `Icon::empty().path("icons/name.svg")`. The name is the
  lucide file name without `.svg`. Check that it exists:
  `ls ~/.cargo/registry/src/*/gpui-kit-assets-0.11.0/assets/icons/<name>.svg`.
- **Style:** size 16px (`.size_4()`), or 14px (`.size_3p5()`) in dense rows. Colour comes from the
  text role, usually `theme.muted_foreground`. Never a colour of their own (`docs/spec/ui.md`).
- **Names in use** (reuse these for the same meaning; add new ones to this table):

| Meaning | Icon |
| --- | --- |
| Database / connection, objects rail, SQL drivers | `database` |
| Table, view, index, trigger, other object | `table`, `eye`, `list`, `zap`, `code` |
| Query tab, saved query | `file-code` |
| Saved queries rail, save query | `bookmark` |
| History rail, queries run | `clock` |
| Database types (count) | `layers` |
| Quick connect | `plug` |
| Connection group (closed / open), new group, move to group | `folder` / `folder-open`, `folder-plus`, `folder-input` |
| Duplicate, copy to clipboard | `copy`, `clipboard` |
| Copy connection URL, copy host | `link`, `server` |
| Copy for spreadsheets, as JSON, as SQL | `sheet`, `braces`, `code` |
| Clone row, keep (undo delete), set NULL, paste | `copy-plus`, `undo-2`, `circle-slash`, `clipboard-paste` |
| See row / document | `eye` |
| Data transfer, swap source and target | `arrow-left-right` |
| New table / collection | `table-2` |
| Disconnect | `unplug` |
| Command palette | `command` |
| Theme toggle | `sun-moon` |
| Settings pages: General, MCP (AI agents) | `sliders-horizontal`, `bot` |
| Settings | `settings` |
| Run | `play` |
| Refresh | `refresh-cw` |
| Export / download | `download` |
| Add / remove a row or condition | `plus` / `minus` |
| Delete, drop | `trash` |
| Truncate (delete all rows) | `eraser` |
| Edit, confirm | `pencil`, `check` |
| Close, back, search | `x`, `arrow-left`, `search` |
| Paging, disclosure | `chevron-left`, `chevron-right`, `chevron-down` |
| Filter builder / raw WHERE toggle | `list-filter` / `square-code` |
| Tiles / list view switch | `layout-grid` / `layout-list` |
| Read-only | `lock` |
| Primary-key mark in grid headers | `key-round` |
