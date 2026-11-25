# Feature spec

Behaviour-level spec of what the app does. It is written in our own words from public
docs and from using SQL clients as a user (see `docs/clean-room.md`). Every row maps to
one or more stable `CommandId`s that the GUI, CLI and MCP all dispatch.

Status: `engine` = in `thaan-core` with tests and reachable from the CLI (`thaan run <id>`), `gui` = also wired
in the GPUI app, `todo` = not started. `engine+gui` means both.

Workspace usability details (connection picker, schema tree, filter row, grid gutter,
status bar) are specified in `docs/spec/workspace.md`; the rows below link to its sections.

## 1. Connections

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Connection list | Saved connections with name, colour label, driver, host/db; grouped in folders | `connection.list` | engine+gui |
| Connection groups | Create and delete groups (folders) of saved connections; move a connection into or out of a group by menu or drag and drop, and reorder by drag; deleting a group keeps its connections (home.md) | `connection.listFolders`, `connection.createFolder`, `connection.deleteFolder`, `connection.moveToFolder` | engine+gui |
| New / edit / delete connection | Form generated from the driver's parameter schema | `connection.save`, `connection.delete` | engine+gui |
| Test connection | Connect, run a trivial query (`ping` for MongoDB), report latency or error | `connection.test` | engine+gui |
| Driver picker | Buttons for every registered driver; the form below is rebuilt from the chosen driver's parameters | — | gui |
| Connect off the UI thread | Connect, Test and schema loading run on a background thread; the buttons show "Connecting…" and are disabled meanwhile, and the window stays responsive | — | gui |
| Connect / disconnect | Opens a session; several sessions can be open at once | `connection.open`, `connection.close` | engine+gui |
| Connection URL import | Paste `postgres://…` / `mysql://…` / `sqlite:…` to fill the form | `connection.parseUrl` | engine+gui |
| SSH tunnel | Host, port, user, password or key file, optional bastion | `connection.sshTunnel` | todo |
| TLS options | Require/verify, CA / client cert / key files | (connection params) | todo |
| Read-only mode | Session refuses writes; UI shows a badge | (connection param `readOnly`) | engine+gui |
| Secrets storage | Passwords stored in the OS keychain, never in the config file | — | todo |

### Drivers

| Driver | Crate | Status |
| --- | --- | --- |
| SQLite | `thaan-driver-sqlite` (rusqlite, feature-gated C) | engine+gui |
| PostgreSQL / CockroachDB / Redshift | `thaan-driver-postgres` (`postgres` crate, pure Rust; TLS todo) | engine+gui (tested live on PostgreSQL 17) |
| MySQL / MariaDB / TiDB | `thaan-driver-mysql` (`mysql` crate, pure Rust; TLS todo) | engine+gui (tested live on MySQL 8.4) |
| MongoDB | `thaan-driver-mongodb` (official `mongodb` crate, sync API) | engine+gui (tested live on MongoDB 7) |
| SQL Server | `thaan-driver-mssql` (tiberius) | todo |
| ClickHouse, DuckDB, LibSQL, Oracle, Firebird, Redis, Cassandra, BigQuery | one crate each | todo |

## 2. Schema browser (sidebar)

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Database / schema picker | Switch database or schema of the active session, from the top of the sidebar (workspace.md §1) | `schema.listDatabases`, `schema.setDatabase` | todo |
| Object tree | Tables, views, materialized views, routines, triggers, sequences; grouped by schema, lazy-loaded (workspace.md §2) | `schema.listObjects` | engine+gui (eager, not lazy) |
| Columns under a table | Expand a table to see columns with types, PK/FK markers (workspace.md §2) | `schema.describeTable` | engine; gui shows them in the Structure view, not in the tree |
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
| Virtualized grid | Millions of rows scroll smoothly; column resize; row-number gutter; headers show type and PK mark (workspace.md §4) | (GUI) | gui (gutter, header types and PK mark done) |
| Row count, duration, affected rows | Shown in the single bottom status bar with paging, refresh and last-fetched time (workspace.md §5) | — | engine+gui |
| Copy | Whole result as CSV / JSON / Markdown / SQL INSERT (cell/selection copy todo) | `result.export` | engine+gui |
| JSON / long text viewer | Side panel shows the selected cell formatted | (GUI) | todo |
| Export | CSV, TSV, JSON, JSON Lines, SQL INSERT, Markdown, Excel | `result.export` | engine (no Excel) |

## 5. Table data editor

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Open table | Paged rows (default 100), server-side sort and filter | `table.fetchPage` | engine+gui (header sort runs on the server for table tabs) |
| Filter builder | Column + operator + value rows, or raw `WHERE` (workspace.md §3) | `table.fetchPage` (filters) | engine+gui (MongoDB tabs keep the raw query box) |
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

## 9. MongoDB

Behaviour written from the MongoDB manual and from using document databases as a user.
No other client's code or binaries were studied (see `docs/clean-room.md`).

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Connect | Connection string (several hosts, options) + user/password stored apart from it; database picked from the field or the URI path | `connection.open` | engine+gui |
| Collections in the sidebar | Collections and views, with their indexes; sidebar heading says "Collections" | `schema.listObjects` | engine+gui |
| Shell-style queries | `db.<coll>.find/findOne/aggregate/countDocuments/distinct/insertOne/insertMany/updateOne/updateMany/replaceOne/deleteOne/deleteMany/createIndex/getIndexes/drop`, `db.createCollection`, `db.runCommand`, `show collections`, `show dbs`; cursor `.sort().skip().limit().projection().count()`; values are relaxed JSON plus `ObjectId()`, `ISODate()`, `NumberInt/Long/Decimal()`, `UUID()`, `/regex/` | `query.runAll`, `query.runCurrent` | engine+gui (JavaScript highlighting) |
| Result grid | One row per document; columns are the union of top-level fields (`_id` first); nested objects/arrays shown as shell text; `find` without `.limit()` returns at most 1000 documents | — | engine+gui |
| Collection view | Paged documents; Filter, Projection and Sort boxes each take a document (`{ age: { $gt: 30 } }`); footer reads "Documents a–b of n" | `table.fetchPage` (`rawWhere`, `projection`, `rawSort`) | engine+gui |
| Result views | Table · Tree · JSON switch for query and collection results; objects/arrays summarised in the table; values coloured by BSON type. Details: `docs/spec/mongodb-ui.md` | — | gui |
| Schema view | Fields inferred from a 100-document sample (types seen, "nullable" when missing or null), indexes; "DDL" is `db.createCollection(...)` + `createIndex(...)` | `schema.describeTable`, `structure.ddl` | engine+gui |
| Edit documents | Staged `$set` / insert / delete by `_id`, previewed as shell commands; applied one by one (no transaction without a replica set) | `table.previewChanges`, `table.applyChanges` | engine (gui editing todo, as for SQL) |
| Read-only | The driver refuses writes, including `$out` / `$merge` aggregations and non-read `runCommand`s | — | engine |

## 10. Headless access

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| CLI | `thaan query/tables/describe/ddl/export/run` | every id above | engine |
| MCP server | Expose the command registry as MCP tools | every id above | todo |
