# AGENTS.md

Guide for coding agents (and humans) working in this repository.

## Goal

- A native, portable (macOS / Windows / Linux), fully open-source **database manager**:
  connect to SQL (and later NoSQL) databases, browse schemas, write and run queries, edit
  table data and structure, import and export.
- The feature set grows toward that of the popular desktop SQL clients, written clean-room
  (see `docs/clean-room.md`). The roadmap is `docs/spec/features.md`.
- Speed and low memory come first (principle 9): everything opens instantly, nothing
  freezes, and the app stays light on RAM.
- The GUI is built with **gpui-kit** (GPUI + its component library). The engine (`thaan-core`) has no
  GUI dependency and stays buildable for `wasm32-unknown-unknown`.

## Principles

1. **Engine-first, headless-first.** Every feature is reachable without a GUI: from tests,
   the CLI (`thaan`) and (later) MCP. The GUI is one client of the engine.
2. **Everything is a command.** Each user-visible action has a stable `CommandId` such as
   `query.runAll` or `table.fetchPage`, with typed, serde-serializable parameters. The
   menu, command palette, shortcuts, CLI, MCP and plugins all dispatch the same commands
   through `thaan_core::commands::dispatch` (the list lives in `builtin_commands()`). Never rename a `CommandId`; deprecate it.
3. **Data describes the UI; toolkits draw it.** Connection forms come from the driver's
   parameter schema (`DriverInfo::params`). Result grids render `ResultSet`. The palette
   lists the registry. A new toolkit implements one renderer for each, not bespoke screens.
4. **One driver trait, one SQL dialect object.** Drivers implement `thaan_core::driver::Driver`
   and `Session`. SQL text generation (quoting, paging, DDL, staged-change SQL) goes
   through `thaan_core::dialect::Dialect`, never through string building in the GUI.
5. **Safe by default.** Writes from the data editor are staged, previewed as SQL and applied in
   one transaction. Read-only connections refuse writes in the engine, not only in the
   UI. Identifiers are always quoted by the dialect; values are always bound parameters.
6. **Determinism and testability.** Engine behaviour is covered by tests that run against
   SQLite in-memory databases. Statement splitting, quoting and export formats have
   golden tests.
7. **Pure Rust by default.** C/C++ dependencies are allowed only behind Cargo features in
   driver or I/O-edge crates (for example `rusqlite/bundled` in `thaan-driver-sqlite`).
8. **Clean-room.** We study other clients' *behaviour* only. Never read, unpack, decompile
   or reverse-engineer another database client's code or binaries (proprietary or GPL,
   including Electron `app.asar` bundles and source maps), and never copy their code,
   CSS, icons, strings or trademarks. Specs come from official database protocol docs,
   public user docs and observed behaviour. See `docs/clean-room.md`.
9. **Fast and light come first.** Speed and low memory outrank features and looks. Opening
   anything must feel instant, the window must never freeze, and the app must stay light on
   the machine and on RAM. When something truly has to wait (connecting to a server, a
   slow query, a big export), the UI says so at once with a loading state. The concrete
   rules are in [Performance](#performance) below and apply to every change.

## Performance

These rules make principle 9 concrete. A change that breaks one is not done.

### Never block the UI thread

- No database, network or disk work on the UI thread. Every `Engine` call that can touch a
  driver, a server or a file (connect, query, `list_objects`, `describe_table`, `ddl`,
  `fetch_page`, export, import, store save) runs in `cx.background_spawn` and updates the
  view when it returns. Only pure in-memory work stays on the UI thread.
- `render` does no I/O and no heavy work. It must not call the engine for data it can
  cache. Cache in the view (history, saved queries, schema) and refresh on change.
- Long work can be cancelled (query cancel, export cancel), and a stale result never
  overwrites newer state: drop the answer when the tab or request it belongs to is gone.

### Show a state for anything that waits

- Under about 100 ms: no indicator, just show the result (a flash of a spinner reads as lag).
- Over about 100 ms: show a state right where the content will appear, said in words:
  `Connecting to shop-prod…`, `Running query…`, `Loading 50 rows…`, `Exporting…`.
  Use a skeleton for grids, lists and the schema tree, a spinner or progress text for
  buttons and the status bar, a progress count for exports and imports.
- Keep the previous content visible while refreshing instead of blanking it.
- Motion for these states follows `docs/spec/ui.md`: subtle, one easing curve, no
  looping decoration on things that are not actually waiting.

### Keep memory small

- Never load a whole table or an unbounded result into memory. Tables are paged
  (`PageRequest`, 50 rows by default in the app). Query results get a row cap with a clear
  "first N rows" note and a way to fetch more.
- Exports and imports stream row by row to and from disk; they never build the whole file
  as one `String` or `Vec`.
- Hold one copy of a result. Share it (`Arc`) between the tab and the grid instead of
  cloning `ResultSet`s. Free a tab's data when the tab closes.
- Grids and long lists are virtualized (render only visible rows). Long cell values are
  truncated for display; the full value loads only when asked for.
- Schema metadata loads lazily (expand a node, then fetch its children) and is cached per
  session.
- History and other stores are capped and saved off the UI thread.

### Start fast, build lean

- Startup opens the window first, then loads anything else. No network at startup.
- Drivers and heavy features stay behind Cargo features; add a dependency only when it
  pays for itself, and prefer the lighter crate.
- Release builds use the tuned `[profile.release]` in the workspace `Cargo.toml`.

### Measure

- Use release builds for any speed or memory claim (`cargo build --release -p thaan-app`).
- Check idle RAM and RAM after opening a large table (Activity Monitor or
  `ps -o rss= -p <pid>` on macOS/Linux), and note the numbers in the commit message when a
  change can affect them.
- Add a test or benchmark for engine hot paths (statement splitting, paging SQL, export
  writers) when changing them.

## Layout

```
crates/
  thaan-core/          engine: commands, connections, dialects, drivers trait,
                       statement splitter, export, history, saved queries. No GUI, no I/O
                       beyond std::fs for the config store.
  thaan-driver-sqlite/ SQLite driver (rusqlite, `bundled` feature)
  thaan-driver-postgres/ PostgreSQL (`postgres` crate; text-format params, binary decode)
  thaan-driver-mysql/  MySQL / MariaDB (`mysql` crate, pure-Rust features)
  thaan-driver-mongodb/ MongoDB (official crate, sync API; `shell.rs` parses shell syntax)
  thaan-keychain/      OS keychain `SecretStore`, shared by the app and the CLI
  thaan-cli/           `thaan` binary: headless access to every command; `thaan mcp`
                       is the MCP server (protocol in `thaan_core::mcp`)
  thaan-app/           GPUI desktop app (gpui-component widgets)
  thaan-web/           browser demo for GitHub Pages (wasm, nightly; docs/web-demo.md)
docs/
  clean-room.md        what sources are allowed
  spec/features.md     behaviour spec + CommandId + status per feature
  spec/ui.md           layout and theme tokens
CONTRIBUTING.md        issues only: no outside pull requests, and why
LICENSE                Apache-2.0
```

## Commands

```bash
cargo build --workspace                      # everything
cargo test --workspace --exclude thaan-app     # engine + drivers + CLI tests (fast, headless)
cargo run -p thaan-cli -- commands             # list every CommandId
cargo run -p thaan-cli -- query sqlite::memory: "select 1"
cargo run -p thaan-cli -- mcp                   # MCP server on stdio (docs/mcp.md)
cargo run -p thaan-app                         # desktop app
scripts/dev-watch.sh                           # desktop app, rebuilt and restarted on save
scripts/build-web.sh                           # browser demo into target/web (nightly)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Adding a feature

1. Add or update the row in `docs/spec/features.md` (behaviour in your own words, CommandId).
2. Implement it in `thaan-core` (or a driver crate), with typed params and a test.
3. Register the command in `thaan_core::commands::builtin_commands()`.
4. Expose it in the CLI if it makes sense headless.
5. Wire the GUI by dispatching the command; do not put logic in `thaan-app`.
6. Update the status column.

## Adding a driver

- New crate `crates/thaan-driver-<name>`, implementing `Driver` + `Session`.
- Provide a `Dialect` (or reuse one) and the `DriverInfo` parameter schema.
- Register it in `thaan-app` and `thaan-cli` behind a Cargo feature named after the driver.
- Tests that need a live server are `#[ignore]` and read the URL from `THAAN_TEST_<NAME>_URL`.
  Throwaway servers for them:

  ```bash
  docker run -d --name thaan-test-pg -e POSTGRES_PASSWORD=test -p 127.0.0.1:55432:5432 postgres:17-alpine
  docker run -d --name thaan-test-mysql -e MYSQL_ROOT_PASSWORD=test -e MYSQL_DATABASE=thaan -p 127.0.0.1:33306:3306 mysql:8.4
  docker run -d --name thaan-test-mongo -p 127.0.0.1:27018:27017 mongo:7
  THAAN_TEST_POSTGRES_URL=postgres://postgres:test@127.0.0.1:55432/postgres \
  THAAN_TEST_MYSQL_URL=mysql://root:test@127.0.0.1:33306/thaan \
  THAAN_TEST_MONGODB_URL=mongodb://127.0.0.1:27018 \
  cargo test --workspace --exclude thaan-app -- --include-ignored
  ```
- Non-SQL drivers set `Dialect::language` to something other than `Sql`; the engine then
  passes editor text through untouched and uses the `Session` hooks (`fetch_page`,
  `apply_changes`, `preview_changes`, `ping`) instead of generating SQL.

## Conventions

- Rust 2021, `anyhow` at binary edges, `thiserror` (`thaan_core::Error`) in libraries.
- No `unwrap()` / `expect()` outside tests, except on invariants with a comment.
- No secrets in logs, config files or test fixtures. Passwords go to the OS keychain
  through `thaan_core::secrets::SecretStore` (the app's `keychain` feature); the store
  file strips them (`store::SECRET_KEYS`). Without a secret store they stay in memory.
- Licence: Apache-2.0 (`LICENSE`). New dependencies use a permissive licence (MIT,
  Apache-2.0, BSD, Zlib, ISC, or MPL-2.0 as a dependency); no GPL / AGPL / LGPL.

## Outside code

The project takes **no outside code**: pull requests are closed without review and
`.github/pull_request_template.md` says so (reasons in `CONTRIBUTING.md`). Bugs and ideas
come in as issues. Never merge, cherry-pick or copy code from a pull request, fork, issue
or patch someone else wrote, even a small one; write the change here from the issue's
description instead.

## Commits

- Commit straight to `main`, one topic per commit; keep refactors apart from behaviour
  changes.
- Messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat(core): add table.truncate command`. Types are `feat`, `fix`, `docs`, `refactor`,
  `perf`, `test`, `build`, `ci`, `chore`; the scope is the crate without `thaan-`
  (`core`, `cli`, `app`, `driver-sqlite`, ...). Breaking changes (a CLI flag, the store
  format, a command's parameters) get `!` (`feat(core)!: ...`) and a line on why.
- Before committing: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace --exclude thaan-app`, and for `thaan-app`
  changes a build and a look at the app.
- Clean-room: every change is written from docs/spec and public docs only
  (`docs/clean-room.md`).
