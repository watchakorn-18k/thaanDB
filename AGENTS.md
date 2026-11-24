# AGENTS.md

Guide for coding agents (and humans) working in this repository.

## Goal

- A native, portable (macOS / Windows / Linux), fully open-source **database manager**:
  connect to SQL (and later NoSQL) databases, browse schemas, write and run queries, edit
  table data and structure, import and export.
- The feature set grows toward that of the popular desktop SQL clients, written clean-room
  (see `docs/clean-room.md`). The roadmap is `docs/spec/features.md`.
- The GUI is built with **GPUI** (`gpui` + `gpui-component`). The engine (`dbm-core`) has no
  GUI dependency and stays buildable for `wasm32-unknown-unknown`.

## Principles

1. **Engine-first, headless-first.** Every feature is reachable without a GUI: from tests,
   the CLI (`dbm`) and (later) MCP. The GUI is one client of the engine.
2. **Everything is a command.** Each user-visible action has a stable `CommandId` such as
   `query.runAll` or `table.fetchPage`, with typed, serde-serializable parameters. The
   menu, command palette, shortcuts, CLI, MCP and plugins all dispatch the same commands
   through `dbm_core::commands::dispatch` (the list lives in `builtin_commands()`). Never rename a `CommandId`; deprecate it.
3. **Data describes the UI; toolkits draw it.** Connection forms come from the driver's
   parameter schema (`DriverInfo::params`). Result grids render `ResultSet`. The palette
   lists the registry. A new toolkit implements one renderer for each, not bespoke screens.
4. **One driver trait, one SQL dialect object.** Drivers implement `dbm_core::driver::Driver`
   and `Session`. SQL text generation (quoting, paging, DDL, staged-change SQL) goes
   through `dbm_core::dialect::Dialect`, never through string building in the GUI.
5. **Safe by default.** Writes from the data editor are staged, previewed as SQL and applied in
   one transaction. Read-only connections refuse writes in the engine, not only in the
   UI. Identifiers are always quoted by the dialect; values are always bound parameters.
6. **Determinism and testability.** Engine behaviour is covered by tests that run against
   SQLite in-memory databases. Statement splitting, quoting and export formats have
   golden tests.
7. **Pure Rust by default.** C/C++ dependencies are allowed only behind Cargo features in
   driver or I/O-edge crates (for example `rusqlite/bundled` in `dbm-driver-sqlite`).
8. **Clean-room.** We study other clients' *behaviour* only. Never read, unpack, decompile
   or reverse-engineer another database client's code or binaries (proprietary or GPL,
   including Electron `app.asar` bundles and source maps), and never copy their code,
   CSS, icons, strings or trademarks. Specs come from official database protocol docs,
   public user docs and observed behaviour. See `docs/clean-room.md`.

## Layout

```
crates/
  dbm-core/            engine: commands, connections, dialects, drivers trait,
                       statement splitter, export, history, saved queries. No GUI, no I/O
                       beyond std::fs for the config store.
  dbm-driver-sqlite/   SQLite driver (rusqlite, `bundled` feature)
  dbm-cli/             `dbm` binary: headless access to every command
  dbm-app/             GPUI desktop app (gpui-component widgets)
docs/
  clean-room.md        what sources are allowed
  spec/features.md     behaviour spec + CommandId + status per feature
  spec/ui.md           layout and theme tokens
```

## Commands

```bash
cargo build --workspace                      # everything
cargo test --workspace --exclude dbm-app     # engine + drivers + CLI tests (fast, headless)
cargo run -p dbm-cli -- commands             # list every CommandId
cargo run -p dbm-cli -- query sqlite::memory: "select 1"
cargo run -p dbm-app                         # desktop app
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Adding a feature

1. Add or update the row in `docs/spec/features.md` (behaviour in your own words, CommandId).
2. Implement it in `dbm-core` (or a driver crate), with typed params and a test.
3. Register the command in `dbm_core::commands::builtin_commands()`.
4. Expose it in the CLI if it makes sense headless.
5. Wire the GUI by dispatching the command; do not put logic in `dbm-app`.
6. Update the status column.

## Adding a driver

- New crate `crates/dbm-driver-<name>`, implementing `Driver` + `Session`.
- Provide a `Dialect` (or reuse one) and the `DriverInfo` parameter schema.
- Register it in `dbm-app` and `dbm-cli` behind a Cargo feature named after the driver.
- Tests that need a live server are `#[ignore]` and read the URL from `DBM_TEST_<NAME>_URL`.

## Conventions

- Rust 2021, `anyhow` at binary edges, `thiserror` (`dbm_core::Error`) in libraries.
- No `unwrap()` / `expect()` outside tests, except on invariants with a comment.
- No secrets in logs, config files or test fixtures. Passwords go to the OS keychain (todo);
  until then they are kept only in memory.
- Keep PRs focused; every PR description states: *"Clean-room: written from docs/spec and
  public docs only."*
