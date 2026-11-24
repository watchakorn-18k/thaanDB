# AGENTS.md

Guide for coding agents (and humans) working in this repository.

## Goal

- A native, portable (macOS / Windows / Linux), fully open-source **database manager**:
  connect to SQL (and later NoSQL) databases, browse schemas, write and run queries, edit
  table data and structure, import and export.
- The feature set grows toward that of the popular desktop SQL clients, written clean-room
  (see `docs/clean-room.md`). The roadmap is `docs/spec/features.md`.
- The GUI is built with **GPUI** (`gpui` + `gpui-component`). The engine (`thaan-core`) has no
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

## Layout

```
crates/
  thaan-core/          engine: commands, connections, dialects, drivers trait,
                       statement splitter, export, history, saved queries. No GUI, no I/O
                       beyond std::fs for the config store.
  thaan-driver-sqlite/ SQLite driver (rusqlite, `bundled` feature)
  thaan-cli/           `thaan` binary: headless access to every command
  thaan-app/           GPUI desktop app (gpui-component widgets)
docs/
  clean-room.md        what sources are allowed
  spec/features.md     behaviour spec + CommandId + status per feature
  spec/ui.md           layout and theme tokens
CONTRIBUTING.md        licence, clean-room, PR and commit rules
LICENSE                Apache-2.0
```

## Commands

```bash
cargo build --workspace                      # everything
cargo test --workspace --exclude thaan-app     # engine + drivers + CLI tests (fast, headless)
cargo run -p thaan-cli -- commands             # list every CommandId
cargo run -p thaan-cli -- query sqlite::memory: "select 1"
cargo run -p thaan-app                         # desktop app
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

## Conventions

- Rust 2021, `anyhow` at binary edges, `thiserror` (`thaan_core::Error`) in libraries.
- No `unwrap()` / `expect()` outside tests, except on invariants with a comment.
- No secrets in logs, config files or test fixtures. Passwords go to the OS keychain (todo);
  until then they are kept only in memory.
- Keep PRs focused; every PR description states: *"Clean-room: written from docs/spec and
  public docs only."*
- Licence: Apache-2.0 (`LICENSE`). PR, commit and sign-off rules are in `CONTRIBUTING.md`;
  the PR template is `.github/pull_request_template.md`.
