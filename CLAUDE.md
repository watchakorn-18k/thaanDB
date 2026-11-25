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
- **Speed and memory come first** (AGENTS.md principle 9 and its Performance section).
  Before finishing any change, check: nothing new runs engine, network or disk work on the
  UI thread; `render` calls no engine method; results are not cloned or loaded unbounded;
  anything that can take over ~100 ms shows a loading state in words.
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
  (overwriting it in place gets the process killed). Do not `codesign` the bundle (the window
  then stops being visible to the tools). Launch with
  `THAAN_HOME=target/thaan-home nohup target/ThaanDB.app/Contents/MacOS/thaan-app &`.
- The app icon and the title-bar mark come from `docs/brand` (see its README). Regenerate
  `AppIcon.icns` from `docs/brand/logo/thaandb-macos-icon*.svg` with `iconutil` if the logo changes.
- Use `THAAN_HOME=target/thaan-home` for manual testing so the user's real store is untouched.
- `pkill -f thaan-app` also matches the shell running the command (its text contains the
  name) and kills it. Use `pkill -x thaan-app`.

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

- **Set:** lucide only (version 1.43, ISC licence in `gpui-kit-assets-0.11.0/LICENSE-LUCIDE`),
  shipped by `gpui-kit-assets`. Do not add other icon sets, emoji or other products' logos.
  The ThaanDB logo is the one exception (`logo.rs`, from `docs/brand`).
- **How:** `icon("name")` in `app.rs` → `Icon::empty().path("icons/name.svg")`. The name is the
  lucide file name without `.svg`. Check that it exists:
  `ls ~/.cargo/registry/src/*/gpui-kit-assets-0.11.0/assets/icons/<name>.svg`.
- **Style:** size 16px (`.size_4()`), or 14px (`.size_3p5()`) in dense rows. Colour comes from the
  text role, usually `theme.muted_foreground`. Never a colour of their own (`docs/spec/ui.md`).
- **Names in use** (reuse these for the same meaning; add new ones to this table):

| Meaning | Icon |
| --- | --- |
| Database / connection, objects rail, SQL drivers | `database` |
| SQLite (a file) / MongoDB (documents) | `file` / `braces` |
| Table, view, index, trigger, other object | `table`, `eye`, `list`, `zap`, `code` |
| Query tab, saved query | `file-code` |
| Saved queries rail, save query | `bookmark` |
| History rail, queries run | `clock` |
| Database types (count) | `layers` |
| Quick connect | `plug` |
| Disconnect | `unplug` |
| Shortcuts card / command palette | `keyboard` / `command` |
| From the terminal (CLI) | `terminal` |
| Theme toggle | `sun-moon` |
| Run | `play` |
| Refresh | `refresh-cw` |
| Export / download | `download` |
| Add / remove a row or condition | `plus` / `minus` |
| Delete | `trash` |
| Edit, confirm | `pencil`, `check` |
| Close, back, search | `x`, `arrow-left`, `search` |
| Paging, disclosure | `chevron-left`, `chevron-right`, `chevron-down` |
| Filter builder / raw WHERE toggle | `list-filter` / `square-code` |
| Read-only | `lock` |
| Primary-key mark in grid headers | `key-round` |
