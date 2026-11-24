# Feature spec

Behaviour-level spec of what the app does. It is written in our own words from public
docs and from using SQL clients as a user (see `docs/clean-room.md`). Every row maps to
one or more stable `CommandId`s that the GUI, CLI and MCP all dispatch.

Status: `engine` = in `dbm-core` with tests and reachable from the CLI (`dbm run <id>`), `gui` = also wired
in the GPUI app, `todo` = not started. `engine+gui` means both.

## 1. Connections

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Connection list | Saved connections with name, colour label, driver, host/db; grouped in folders | `connection.list` | engine+gui (folders todo) |
| New / edit / delete connection | Form generated from the driver's parameter schema | `connection.save`, `connection.delete` | engine+gui |
| Test connection | Connect, run a trivial query, report latency or error | `connection.test` | engine+gui |
| Connect / disconnect | Opens a session; several sessions can be open at once | `connection.open`, `connection.close` | engine+gui |
| Connection URL import | Paste `postgres://…` / `mysql://…` / `sqlite:…` to fill the form | `connection.parseUrl` | engine+gui |
| SSH tunnel | Host, port, user, password or key file, optional bastion | `connection.sshTunnel` | todo |
| TLS options | Require/verify, CA / client cert / key files | (connection params) | todo |
| Read-only mode | Session refuses writes; UI shows a badge | (connection param `readOnly`) | engine+gui |
| Secrets storage | Passwords stored in the OS keychain, never in the config file | — | todo |

### Drivers

| Driver | Crate | Status |
| --- | --- | --- |
| SQLite | `dbm-driver-sqlite` (rusqlite, feature-gated C) | engine+gui |
| PostgreSQL / CockroachDB / Redshift | `dbm-driver-postgres` (tokio-postgres, pure Rust) | todo |
| MySQL / MariaDB / TiDB | `dbm-driver-mysql` (mysql_async, pure Rust) | todo |
| SQL Server | `dbm-driver-mssql` (tiberius) | todo |
| ClickHouse, DuckDB, LibSQL, Oracle, Firebird, MongoDB, Redis, Cassandra, BigQuery | one crate each | todo |

## 2. Schema browser (sidebar)

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Database / schema picker | Switch database or schema of the active session | `schema.listDatabases`, `schema.setDatabase` | todo |
| Object tree | Tables, views, materialized views, routines, triggers, sequences; lazy-loaded | `schema.listObjects` | engine+gui (eager, not lazy) |
| Columns under a table | Expand a table to see columns with types, PK/FK markers | `schema.describeTable` | engine; gui shows them in the Structure view, not in the tree |
| Filter | Type to filter the tree by name | (GUI-local) | gui |
| Pin / favourite tables | Pinned items listed at the top | `schema.pin` | todo |
| Context menu | Open data, open structure, copy name, generate SELECT/INSERT, drop, truncate, export | (dispatches other commands) | todo |
| Refresh | Reload schema metadata | `schema.refresh` | engine+gui |

## 3. Query editor

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Query tabs | Many tabs, each bound to a connection; rename, reorder, close others | `tab.new`, `tab.close` | gui |
| Run all | Execute every statement; one result pane per statement | `query.runAll` | engine+gui |
| Run current / selection | Statement under cursor, or the selection | `query.runCurrent`, `query.runSelection` | engine+gui |
| Statement splitting | Respects quotes, comments, dollar-quoting, `DELIMITER` | `query.split` | engine |
| Cancel | Cancel the running query | `query.cancel` | todo |
| Query parameters | `:name` / `$1` / `?` placeholders prompt for values before running | `query.params` + `params` on `query.run*` | engine (CLI `--param`); GUI prompt todo |
| Syntax highlighting | SQL highlighting per dialect | (GUI) | gui |
| Autocomplete | Keywords, tables, columns, aliases | `editor.complete` | todo |
| Format SQL | Pretty-print the buffer or selection | `query.format` | todo |
| Saved queries | Save with a name and folder; open, rename, delete | `savedQuery.*` | engine+gui (no folders/rename yet) |
| History | Every run stored with time, duration, row count, connection | `history.list`, `history.clear` | engine+gui |
| AI assistant | Ask in natural language; generates SQL against the schema. Opt-in, user-supplied API key | `ai.ask` | todo |

## 4. Results grid

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Virtualized grid | Millions of rows scroll smoothly; column resize | (GUI) | gui |
| Row count, duration, affected rows | Shown in the result status bar | — | engine+gui |
| Copy | Whole result as CSV / JSON / Markdown / SQL INSERT (cell/selection copy todo) | `result.export` | engine+gui |
| JSON / long text viewer | Side panel shows the selected cell formatted | (GUI) | todo |
| Export | CSV, TSV, JSON, JSON Lines, SQL INSERT, Markdown, Excel | `result.export` | engine (no Excel) |

## 5. Table data editor

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Open table | Paged rows (default 100), server-side sort and filter | `table.fetchPage` | engine+gui (header sort is client-side in gui) |
| Filter builder | Column + operator + value rows, or raw `WHERE` | `table.fetchPage` (filters) | engine; gui has the raw `WHERE` box only |
| Inline edit | Edit cells, add rows, delete rows; changes staged and highlighted | `table.stageChange` | todo |
| Apply / discard | Apply staged changes in one transaction, or preview the SQL | `table.applyChanges`, `table.previewChanges` | engine |
| Set NULL / default | Per-cell actions | (staged change) | todo |

## 6. Table structure editor

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Columns | Add, rename, change type, nullable, default; preview DDL before applying | `structure.alterColumns` | todo |
| Indexes | List, create, drop | `structure.indexes` | todo |
| Foreign keys | List, create, drop | `structure.relations` | todo |
| Triggers | List, view definition | `structure.triggers` | todo |
| Create table | Form-based table creation with DDL preview | `structure.createTable` | todo |
| DDL view | Show `CREATE` statement for any object | `structure.ddl` | engine+gui (SQLite) |

## 7. Import / export / backup

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Export table(s) | Whole table or filtered rows, many formats, to file | `export.table` | engine+gui (CSV/JSON to ~/Downloads; file picker todo) |
| Import CSV | Map CSV columns to table columns, preview, insert in batches | `import.csv` | todo |
| Backup / restore | Wrap native dump tools when present (`pg_dump`, `mysqldump`) | `backup.*` | todo |

## 8. Workspace & app

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Command palette | Fuzzy search over every registered command | `app.commandPalette` | gui |
| Keyboard shortcuts | Configurable, stored in settings | — | todo |
| Themes | Light / dark toggle (system mode todo) | `app.toggleTheme` | gui |
| Multiple windows | One window per workspace | — | todo |
| Team workspaces / sync | Optional self-hostable sync of connections and saved queries (no secrets) | `workspace.*` | todo |
| Entity-relationship diagram | Auto-layout diagram of tables and FKs | `schema.erDiagram` | todo |

## 9. Headless access

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| CLI | `dbm query/tables/describe/ddl/export/run` | every id above | engine |
| MCP server | Expose the command registry as MCP tools | every id above | todo |
