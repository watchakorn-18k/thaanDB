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
| Connection list | Saved connections with name, colour label, driver, host/db; grouped in folders. The colour is picked freely in the connection form | `connection.list` | engine+gui |
| Connection colour | The open session's colour fills the bottom status bar, text in black or white for contrast | (connection field `color`) | gui |
| Connection groups | Create and delete groups (folders) of saved connections; move a connection into or out of a group by menu or drag and drop, and reorder by drag; deleting a group keeps its connections (home.md) | `connection.listFolders`, `connection.createFolder`, `connection.deleteFolder`, `connection.moveToFolder` | engine+gui |
| Duplicate connection | Save a copy named `<name> copy` (or a given name) right below the original; its password is copied through the keychain, so the copy connects even after a restart | `connection.duplicate` | engine+gui+cli+mcp |
| New / edit / delete connection | Form generated from the driver's parameter schema | `connection.save`, `connection.delete` | engine+gui |
| Test connection | Connect, run a trivial query (`ping` for MongoDB), report latency or error | `connection.test` | engine+gui |
| Driver picker | Buttons for every registered driver; the form below is rebuilt from the chosen driver's parameters | — | gui |
| Connect off the UI thread | Connect, Test and schema loading run on a background thread; the buttons show "Connecting…" and are disabled meanwhile, and the window stays responsive | — | gui |
| Connect / disconnect | Opens a session; several sessions can be open at once. The sidebar lists every saved connection; clicking one connects it without closing the others, and an open one shows a green dot and its logo in its colour (workspace.md §1a) | `connection.open`, `connection.close` | engine+gui |
| Connection URL import | Paste `postgres://…` / `mysql://…` / `sqlite:…` to fill the form | `connection.parseUrl` | engine+gui |
| SSH tunnel | Host, port, user, password or key file, optional bastion | `connection.sshTunnel` | todo |
| TLS options | `sslmode` as in libpq: `prefer` (default: TLS when offered, plain otherwise), `disable`, `allow`, `require` (encrypt, certificate not checked), `verify-ca` (trusted chain), `verify-full` (trusted chain and host name); `sslrootcert` adds a CA file to the system roots. Pasting a URL fills the form fields, `?sslmode=` included | (connection params `sslmode`, `sslrootcert`) | engine+gui on PostgreSQL (rustls; client certificates todo; MySQL todo) |
| Read-only mode | Session refuses writes; UI shows a badge | (connection param `readOnly`) | engine+gui |
| Secrets storage | Passwords (and SSH secrets) stored in the OS keychain, never in the config file: Keychain on macOS, Credential Manager on Windows, Secret Service on Linux (`keychain` feature of the app). Save writes them; a successful connect with a typed password remembers it; an empty password box means "use the stored one", and the edit form fills it from the keychain; deleting a connection removes them. The keychain is read at most once per connection and key per run (cached in memory), so its system prompt does not repeat on every connect. On macOS the app reads and writes these entries through Apple's `/usr/bin/security` tool, so no Keychain prompt appears, also after a rebuild or update; the trade-off is that any program running as the same user can read them through that tool | — (`Engine::with_secrets`, `secrets::SecretStore`) | engine+gui (CLI todo) |

### Drivers

| Driver | Crate | Status |
| --- | --- | --- |
| SQLite | `thaan-driver-sqlite` (rusqlite, feature-gated C) | engine+gui |
| PostgreSQL / CockroachDB / Redshift | `thaan-driver-postgres` (`postgres` crate, rustls for TLS) | engine+gui (tested live on PostgreSQL 17) |
| MySQL / MariaDB / TiDB | `thaan-driver-mysql` (`mysql` crate, pure Rust; TLS todo) | engine+gui (tested live on MySQL 8.4) |
| MongoDB | `thaan-driver-mongodb` (official `mongodb` crate, sync API) | engine+gui (tested live on MongoDB 7) |
| Redis | `thaan-driver-redis` (own RESP2 client on `std::net`, no TLS yet) | engine+gui (tested live on Redis 7) |
| MQTT 3.1.1 | `thaan-driver-mqtt` (own client on `std::net`, no TLS yet) | engine+gui (tested live on Mosquitto 2) |
| SQL Server | `thaan-driver-mssql` (tiberius) | todo |
| ClickHouse, DuckDB, LibSQL, Oracle, Firebird, Cassandra, BigQuery | one crate each | todo |
| Kafka, RabbitMQ (AMQP), NATS | one crate each, same model as MQTT | todo |

## 2. Schema browser (sidebar)

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Database / schema picker | Every database of the server sits in the sidebar tree (connection → database → folders → objects); opening an object in another database opens a second session for that database and keeps the first (workspace.md §1a) | `schema.listDatabases`, `schema.setDatabase` | engine+gui |
| Object tree | Tables, views, materialized views, routines, triggers, sequences; grouped by schema, lazy-loaded (workspace.md §2) | `schema.listObjects` | engine+gui (eager, not lazy) |
| Columns under a table | Expand a table to see columns with types, PK/FK markers (workspace.md §2) | `schema.describeTable` | engine; gui shows them in the Structure view, not in the tree |
| Schema overview | Every table and view of a database with its columns (type, PK, NOT NULL, default, foreign key target) and the names of routines, in one call; capped at 200 tables by default | `schema.context` | engine+cli+mcp |
| Filter | Type to filter the tree by name | (GUI-local) | gui |
| Pin / favourite tables | Pinned items listed at the top | `schema.pin` | todo |
| Context menu | Right-click a table, view or collection: open data or structure, new query, copy (name, qualified name, SELECT, CREATE), refresh, export (CSV, JSON, SQL INSERT), rename, duplicate, truncate, drop. Rename / duplicate / truncate / drop open one dialog that asks first; truncate and drop say they cannot be undone and use the danger button; read-only connections refuse them in the engine (workspace.md §1a) | `table.previewOp`, `table.rename`, `table.duplicate`, `table.truncate`, `table.drop` (+ commands it dispatches) | engine+gui+cli (export still writes on the UI thread, gap 2) |
| Refresh | Reload schema metadata | `schema.refresh` | engine+gui |
| New table / collection | Right-click a database: *Add collection* (MongoDB, by name) or *New table* (a CREATE TABLE template in the database's dialect, opened in a query tab on that database) | `collection.create` | engine+gui+cli |

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
| Copy | Whole result as CSV / JSON / Markdown / SQL INSERT; one cell or one row: click a row number to select the row, ⌘C copies it as TSV (pastes into Excel / Google Sheets cells; NULL is an empty cell), and right-clicking the row number offers Copy row, Copy row with column names, Copy row as JSON and Delete row (multi-row selection todo) | `result.export` | engine+gui |
| Cell and row menu | Right-click a cell: Set to NULL; Quick filter ▸ (= value, ≠ value, contains for text, > and < otherwise, is NULL / is not NULL); Copy value (⌘C), Copy column name; Paste (⌘V, the first value on the clipboard, staged like an edit) and Edit cell; then the row actions, which a row number's menu has too: copy the row for spreadsheets (TSV, with or without column names), as JSON, as Markdown or as SQL INSERT; Clone row (⌘D: a copy at the end of the grid, every value but the primary key, inserted on Apply; on MongoDB a prefilled Insert document dialog without `_id`); Delete row (⌫, staged, toggles back); See row / See document. Menu rows show their shortcut keys | `table.stageChange`, `result.export` | gui |
| Rows per page | A box in the status bar: type any size (1–1000) or pick 1, 5, 10, 25, 50, 100 or 200 from its list; going back to page 1 and reloading. 50 when a table opens | `table.fetchPage` (`pageSize`) | gui |
| JSON / long text viewer | Side panel shows the selected cell formatted | (GUI) | todo |
| Export | CSV, TSV, JSON, JSON Lines, SQL INSERT, Markdown, Excel | `result.export` | engine (no Excel) |

## 5. Table data editor

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Open table | Paged rows (default 100), server-side sort and filter | `table.fetchPage` | engine+gui (header sort runs on the server for table tabs) |
| Filter builder | Column + operator + value rows, or raw `WHERE` (workspace.md §3) | `table.fetchPage` (filters) | engine+gui (MongoDB tabs keep the raw query box) |
| Inline edit | Double-click a cell to edit (relational tables); right-click a row to stage its deletion (every database, row shown red); edited cells are tinted; Reset / N Apply in the status bar. Add row: the + in the status bar (or ⌘N) adds an empty row at the end of the grid, marked "+" in the gutter, with its first non-key cell open for typing; only typed cells are inserted on Apply, so column defaults fill the rest; Discard removes it. On MongoDB the same + opens the Insert document dialog. Row bar (SQL tables, like MongoDB's document bar): under the filter, ghost icon buttons Add row · View row (a dialog of column / value pairs) · Edit cell (opens the selected cell, or the first non-key one) · Delete row (staged, toggles back), with "Row N" / "New row" / "No row selected" on the right; on read-only connections only View works | `table.stageChange`, `table.applyChanges` | gui |
| Command box | Table tabs open a resizable command editor with line numbers and a ready command (`SELECT … LIMIT 100` or `db.getCollection(…).find({})`); ⌘↵ runs it into the grid; hideable | `query.runAll` | gui |
| Apply / discard | Apply staged changes in one transaction, or preview the SQL | `table.applyChanges`, `table.previewChanges` | engine |
| Set NULL / default | Per-cell actions | (staged change) | todo |

## 6. Table structure editor

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Columns | List (name, type, nullable, default, comment, primary-key position) in the Structure view's Columns tab; add (name, type, nullable, default), drop, and rename / change type, NULL and default in one form, each with the exact SQL shown first. Only what changed is altered; MySQL restates the column with its comment and AUTO_INCREMENT kept; on PostgreSQL and SQLite a change of several statements runs in one transaction. SQLite can rename but not change a type, NULL or default | `schema.describeTable`; `structure.preview`, `structure.apply` (`addColumn`, `dropColumn`, `alterColumn`) | engine+gui+cli |
| Indexes | List (name, unique, primary, columns); create (name, ordered columns, unique) and drop (not the primary key's index), SQL shown first | `schema.describeTable` (`indexes`); `structure.preview`, `structure.apply` (`createIndex`, `dropIndex`) | engine+gui+cli (SQLite, PostgreSQL, MySQL) |
| Foreign keys | List (name, columns, referenced schema/table/columns, ON UPDATE, ON DELETE); add (name optional, columns, referenced table and columns, actions) and drop, SQL shown first. SQLite cannot add or drop one on an existing table and says so | `schema.describeTable` (`foreignKeys`); `structure.preview`, `structure.apply` (`addForeignKey`, `dropForeignKey`) | engine+gui+cli (PostgreSQL, MySQL; SQLite list only) |
| Triggers | List (name, timing, event, action, WHEN condition); full definition in the DDL tab | `schema.describeTable` (`triggers`) | engine+gui (SQLite, PostgreSQL, MySQL) |
| Create table | Form-based table creation with DDL preview | `structure.createTable` | todo |
| DDL view | Show `CREATE` statement for any object | `structure.ddl` | engine+gui (SQLite) |

## 7. Import / export / backup

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Export table(s) | Whole table or filtered rows, many formats, to file | `export.table` | engine+gui (CSV/JSON to ~/Downloads; file picker todo) |
| Import CSV | Map CSV columns to table columns, preview, insert in batches | `import.csv` | todo |
| Backup / restore | Wrap native dump tools when present (`pg_dump`, `mysqldump`) | `backup.*` | todo |
| Data transfer | Copy tables or collections from one connection and database to another, any engine to any engine (workspace.md §9). Copy structure and rows, structure only, or rows only into existing tables; an existing target table is appended to, emptied first, or dropped and recreated. Structure comes from the source's columns with types mapped to the target's dialect (keys and NOT NULL kept; defaults, indexes and foreign keys not). Rows go one page (500) at a time, so memory stays at one page; progress after every batch; Stop ends it at the next batch; a read-only target is refused. Opened from a database's right-click menu (it starts from that database) or the palette | `data.transfer` | engine+gui (saved transfer tasks todo) |

## 8. Workspace & app

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Command palette | Fuzzy search over every registered command | `app.commandPalette` | gui |
| Keyboard shortcuts | Configurable, stored in settings | — | todo |
| Themes | Light / dark / system, saved; toggle in the rail | `app.toggleTheme` | gui |
| Settings | Dialog: theme, text size, rows per page, history size; saved in the store | `app.settings`, `settings.get`, `settings.set` | done |
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

## 9a. Redis

Written from the Redis protocol spec and command reference. No other client's code was studied.

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Connect | Host, port, optional ACL user, password, database number; `redis://:pw@host:6379/2` fills the form | `connection.open` | engine+gui |
| Databases | `db0`…`dbN` (from `CONFIG GET databases`, 16 when the server refuses CONFIG) | `schema.listDatabases` | engine+gui |
| Key browser | A database row in the tree opens a key browser tab (no folders under it). Left: keys grouped into folders by `:` with counts (folders first, collapsed until opened); each key shows its type and TTL as small badges; checkboxes on keys and folders, *Select all*, *Delete N* (asks first); pattern box (`SCAN MATCH`) with *Fuzzy* (a plain word matches anywhere) and *No expiry*; type picker (`SCAN TYPE`); `+` adds a key (name, type, first value or field/score, optional expiry; refuses a name that exists). Keys stream in (500 per `SCAN` step, TYPE and TTL pipelined) until the scan ends or 10,000 are kept; the header says `N / M keys loaded`. Right: the key name with type, size and TTL badges; reload, copy name, delete; for hashes, sets and sorted sets an item search (`MATCH`); an add row (*Push* to head or tail, *Add field*, *Add member* with score); the items a page at a time. *Rename key* (pencil; `RENAMENX`, refuses a name that exists). Double-click an item, or select it and press *Open item*, to open it in a dialog: its index / field / member with UTF-8 or binary, size and score badges; View as Text, JSON (indented), Hex (16 bytes a line with offsets) or Base64; Word wrap; *Copy*; *Edit* swaps in an editor and *Save* writes it as a staged change (`LSET`, `HSET`, `SET … KEEPTTL`, or the old member replaced in a set or sorted set, in one MULTI / EXEC). Stream entries and binary values are read-only there; binary values open in Hex. A *Decode* row first turns the stored bytes into readable ones: GZip, Zlib, Deflate, Base64 (bytes), MessagePack, Pickle, Protobuf without a schema (indented JSON); output capped at 16 MB; editing only with Decode set to None (`thaan_core::decode`, core feature `decoders`). The clock in the key header sets the expiry (`EXPIRE`) or removes it (`PERSIST`). A *Command line* tab next to *Key* runs any command on that database without adding to history, printing replies as aligned columns (500 rows each, last 200 commands kept); read-only connections refuse writes there too. Other databases open on a second session. (table-view.md T6) | `keys.scan`, `table.fetchPage`, `table.applyChanges`, `session.command` | engine+gui |
| Keys as objects | `schema.listObjects` still returns the first 1000 keys `SCAN` finds, for the CLI and MCP | `schema.listObjects` | engine+cli |
| Key view | Rows by type: string `value`; hash `field · value`; list `index · value`; set `member`; sorted set `member · score` (sort score descending uses `ZREVRANGE`); stream `id · fields`. Paged with `LRANGE` / `ZRANGE` / `XRANGE` / `*SCAN`, never read whole. Filter rows take `=` or `like` on the first column (becomes `MATCH`), the raw box takes a glob | `table.fetchPage` | engine+gui |
| Commands | One command per line (quotes and `\xHH` escapes); replies shaped as tables (`HGETALL` → field/value, `INFO` → section/key/value, `SCAN` shows the next cursor in the header); results capped at 10,000 rows; `SUBSCRIBE` / `MONITOR` refused | `query.runAll`, `query.runCurrent` | engine+gui |
| Edit keys | Staged edits become `SET … KEEPTTL`, `HSET` / `HDEL`, `LSET` / `RPUSH` / `LREM`, `SADD` / `SREM`, `ZADD` / `ZREM`, `XADD` / `XDEL`, previewed as commands and run in one `MULTI` / `EXEC` | `table.previewChanges`, `table.applyChanges` | engine+gui |
| Key operations | Rename is `RENAMENX` (never overwrites), drop is `DEL`, duplicate is `COPY`; "DDL" lists type, size, TTL, encoding and the commands that recreate the first 100 items | `table.rename`, `table.drop`, `table.previewOp`, `structure.ddl` | engine+gui |
| Read-only | Only read commands run (`GET`, `HGETALL`, `SCAN`, `INFO`, `CONFIG GET`…) | — | engine |

## 9b. MQTT

Written from the MQTT 3.1.1 OASIS standard. No other client's code was studied.

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Connect | Host, port, user, password, topic filter to subscribe to (`#` by default), client id, messages kept (1000 by default); keep-alive pings in the background | `connection.open` | engine+gui |
| Console | The connection's one row in the tree, *Console*, opens a console tab. Left: *Subscribe to a topic filter* at the top (filter, QoS 0/1 segmented, *Subscribe*); then *All messages* and each subscription with its QoS and message count, which narrows the list to what it matches; `x` unsubscribes. Middle: messages received and sent, newest first, polled every 500 ms from the session buffer (ids after the last one seen), with All / Received / Sent, a topic and payload search, Pause, Clear and a *Publish* toggle. Right, when a message is picked: its topic, time, direction, QoS, size, retained, and the payload in full (JSON indented) with *Copy* and *Publish to this topic*. The publish panel (hidden until asked for): labelled *Topic* and *Payload*, QoS 0/1, *Retain*, *Publish* (Enter in the topic publishes). Times are local. (table-view.md T7) | `broker.messages`, `broker.subscriptions`, `session.command` | engine+gui |
| Topics as objects | `schema.listObjects` returns the subscribed filter and every topic seen, for the CLI and MCP | `schema.listObjects` | engine+cli |
| Topic view | Buffered messages, newest first: `received_at · topic · payload · qos · retain · bytes`; a wildcard object (`a/+/t`) shows every match; filter rows run in memory on the bounded buffer; payloads over 256 KiB are cut | `table.fetchPage` | engine+gui |
| Commands | `PUBLISH topic payload [QOS 0\|1] [RETAIN]`, `SUBSCRIBE filter [QOS n]`, `UNSUBSCRIBE filter`, `MESSAGES [filter] [limit]`, `TOPICS`, `CLEAR`, `HELP` | `query.runAll` | engine+gui |
| Publish from the grid | Adding a row publishes it (topic defaults to the open topic); deleting a retained row clears it (empty retained publish); messages cannot be edited | `table.applyChanges` | engine+gui |
| Read-only | Refuses `PUBLISH` and grid writes | — | engine |

## 9c. Browser demo

Written from the WebAssembly, WebSocket and MQTT specs; see `docs/web-demo.md`.

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| Demo page | ThaanDB in a web page (GitHub Pages): a sample shop database (SQLite in memory) with its tables in the desktop grid, a SQL editor, an MQTT console over WebSocket, and a link to the desktop app. No servers over TCP, no MCP, no files | — | web (built, `crates/thaan-web`) |

## 10. Headless access

| Feature | Behaviour | CommandId | Status |
| --- | --- | --- | --- |
| CLI | `thaan query/tables/describe/ddl/export/run` | every id above | engine |
| MCP server | `thaan mcp` speaks MCP over stdio (see `docs/mcp.md`). Tools: `list_connections`, `add_connection`, `duplicate_connection`, `query`, `list_databases`, `list_tables`, `get_schema_context`, `describe_table`, `fetch_rows`, `list_routines`, `get_routine_source`, `open_table` and `execute_and_show` (show it in the running app), `disconnect`, plus `list_commands` / `run_command` for every id above. `connection` is a saved name or id or a URL and opens on first use; no permission setup, read-only connections stay read-only in the engine. Connection passwords never reach the agent: every reply and error is redacted. Query replies are capped (200 rows by default, `maxRows` up to 5000), long text cells shortened. Settings › MCP shows the server path and a ready config for Claude Code, Claude Desktop, Cursor, VS Code, Codex and Windsurf | every id above | engine+gui |
