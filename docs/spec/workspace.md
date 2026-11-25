# Workspace usability spec

How the main workspace should feel to use: connection picker, object tree, filter row,
grid gutter and headers, status bar. Written in our own words.

**Source type:** observed behaviour. A screenshot of another desktop SQL client in normal
use (sidebar, table tab and status bar), shared by the maintainer on 2026-11-25. Nothing
was read from that client's code or bundle (see `docs/clean-room.md`). We take the
*workflow* only. Colours, icons, wording and pixel layout stay ours (`docs/spec/ui.md`).

Each section says what the user sees and does, which engine command backs it, and what
is new. Commands marked *new* need a row in `features.md`, typed params, a test and a
registration in `builtin_commands()` before the GUI wires them.

---

## 1. Connection picker in the sidebar

**Why:** while connected you can't currently tell which database you're in without
looking at the title bar, and switching databases means disconnecting.

**Behaviour**
- The top of the sidebar shows the active session: connection dot, connection name, and
  (where the driver has more than one) the current database.
- Clicking it opens a list of the session's databases, so you can switch without
  reconnecting. The tree reloads for the chosen database.
- Two icon buttons sit next to it: *Refresh* (reloads the schema) and *New* (opens a
  menu of New query, and later New table).
- On SQLite (one database per file) the picker shows the file name and has no list.

**Commands:** `schema.listDatabases`, `schema.setDatabase` (exist as ids, status todo),
`schema.refresh`, `tab.new`.

### 1a. One page, several connections open at once

The window is always one page: the sidebar tree on the left, the tab strip on the
right with **Home** as its first tab. Nothing changes page when you connect, open a
table or switch connection; Home shows the overview (saved connections, quick connect,
recent queries) in the main area whenever it is the selected tab or no tab is open.
**Double-click a connection to connect it, and that is all**: its tree row gets the
green dot and unfolds, no tab opens and the main area stays as it was. A single click
on a closed connection does nothing (its chevron connects on one click). Tables and
collections are then opened straight from the tree, each as a tab on the same page.
**Right-click → Open** is the only action that opens a page for a connection: a new
query tab for it (connecting first if needed). The Quick connect rows on Home work the
same way (double-click connects, the row says Connect / Connecting… / Connected).

One tab strip holds the tabs of every open connection; each tab starts with its
connection's colour dot. Showing a tab brings its connection to the front (title bar,
status bar colour, Run), so they always match what is on screen. Closing a connection
closes its tabs. Clicking a table opens it as a tab right there. A table of another
database of the same server opens as a tab named `table · database`, on a second
session opened for that database in the background; the connection's own session and
tabs stay, and that database gets the green dot too. Those extra sessions close with
the connection. Right-click a closed connection in the tree for the same menu as on
Home (Connect, Edit, Duplicate, Copy, Move to group, New group, Delete); *New group*
also sits next to the filter.


**Why:** comparing two servers, or hopping from a dev database to prod, should not mean
disconnecting and reconnecting each time.

**Behaviour**
- While connected, the sidebar tree lists every saved connection, in its groups, with
  the open ones unfolded to their databases and objects.
- Clicking a closed connection connects it; the connections already open stay open.
  Clicking an open one brings it to the front (its own tabs, title and status bar)
  and unfolds it; only the chevron folds, so a double-click never undoes itself.
- An open connection shows its database logo in its own colour and a 6px green dot after
  the name. Under it, the database the session is on gets the same colour and dot; the
  other databases stay muted until opened. Closed connections stay muted.
- The connection in front has the selected row background and a medium-weight name.
- Switching connection is an in-memory swap: nothing reconnects or reloads. A query or page that finishes while its connection is behind
  another lands in its own tab, never in the one in front.
- Right-click an open connection: *New query*, *Refresh*, *Disconnect*. Right-click a
  closed one: *Connect* (opens its page in this window) or *Edit connection*. The rail's *Disconnect all* button (unplug icon) closes every open
  connection and its tabs and shows Home; right-click → *Disconnect* closes just one.

Right-click a database: *New query* (a query tab on that database; for another
database than the connection's own it opens a second session first and the tab reads
`Query 3 · shop`), *Add collection* on MongoDB (a name box, then
`db.createCollection(…)` on that database) or *New table* elsewhere (a query tab on that
database with a CREATE TABLE in its dialect, an identity key, a name and a creation
time, to edit and run; the tree refreshes after DDL), *Refresh*, *Copy name*, and *Close database connection*, which closes
that second session and its tabs (greyed for the connection's own database, which closes
with the connection).

Right-click a table, view or collection: *Open data*, *Open structure* (not for
collections), *New query* (a query tab with `SELECT * FROM … LIMIT 100`, or
`db.getCollection(…).find({})` on MongoDB), *Copy ▸* (Name, Qualified name or
collection expression, SELECT statement or find() command, CREATE statement read from
the server off the UI thread), *Refresh*, *Export ▸* (CSV, JSON, SQL INSERT: opens the
tab and exports what it shows), then *Rename*, *Duplicate* (tables, with or without
rows) and, in the danger colour, *Truncate* (tables) and *Drop*. Menu labels carry no
"…". These four open one dialog: a new-name box where needed (the button stays off
while the name is empty or unchanged, with no error text), a short note only when the
database cannot do it at all (SQLite views cannot be renamed), and for truncate and
drop a red "This can't be undone" line and a danger button; nothing is preselected.
The SQL itself is not shown (`table.previewOp` gives it headless). They run off the UI thread (on the database's own session for objects of
another database); afterwards tabs of a dropped or renamed object close, a truncated
table's open tab reloads, and the tree refreshes. New query and CREATE statement work on
the connection's own database; for tables of another database they are greyed.

**Commands:** `connection.open`, `connection.close`, `tab.new`, `schema.refresh`.

## 2. Object tree grouped by schema

**Why:** on PostgreSQL there can be hundreds of tables across several schemas. A flat
list grouped only by kind doesn't scale.

**Behaviour**
- A header line reads `Objects` followed by the total count in muted mono, plus an
  add button on the right (New table, todo until `structure.createTable`).
- When the driver has schemas, the first level is the schema (folder icon, expanded by
  default for the default schema such as `public`). On SQLite there is no schema level.
- Inside a schema, objects are listed by name with a kind icon (table, view,
  materialized view). Indexes and triggers move under their table instead of being a
  top-level group.
- Every table row has a disclosure chevron. Expanding it shows the columns: name, type
  in muted mono, and a key mark on primary key columns. Columns load lazily on first
  expand (`schema.describeTable`) and are cached until refresh.
- Clicking a table row opens its data tab (as today). The open table's row is
  highlighted in the tree, and switching tabs moves that highlight.
- The filter box filters by name across every schema and keeps the parents of matches
  visible.
- Rows keep the 28px height and 4px rhythm from `ui.md`. Indent is 16px per level.

**Commands:** `schema.listObjects` (needs the object's schema filled in,
`ObjectRef.schema` already exists), `schema.describeTable`, new GUI-local state for
which nodes are expanded.

## 3. Filter row in the table tab

**Why:** typing a raw `WHERE` clause is slow for the common case "column equals value".
The engine already supports structured filters (`PageRequest.filters`, `FilterOp`). The
GUI just doesn't expose them.

**Behaviour**
- Below the tab strip a single filter row has three parts: a **column** picker (the
  table's columns), an **operator** picker (`=`, `!=`, `<`, `<=`, `>`, `>=`, `like`,
  `not like`, `is null`, `is not null`, `in`), and a **value** box.
- Enter in the value box, or the search button at the right, applies the filter and goes
  to page 1. The value box is hidden for `is null` / `is not null`. For `in`, the value is
  a comma-separated list.
- A *+* button adds another row. Rows are combined with AND. Each extra row has a remove
  button.
- A toggle at the left switches between this builder and the raw `WHERE` box (today's
  behaviour). The text in either mode survives the switch.
- An empty value with an operator that needs one means no filter. It never sends `= ''`.
- Values are always bound parameters (principle 5); the column is quoted by the dialect.
- Operators are listed in plain words (`equals`, `greater than`, `is one of`, …) from
  `FilterOp::title`; the value is typed by the column's declared type (`Filter::from_text`).
- **Status:** built. MongoDB tabs and tables without known columns show only the raw box.

**Commands:** `table.fetchPage` with `filters` (engine done). No new command.

## 4. Grid gutter and column headers

**Behaviour**
- A narrow gutter on the left of the grid shows the row number in muted mono, counted
  across pages (page 2 of 50 rows starts at 51). It stays fixed while scrolling
  horizontally and doesn't scroll away.
- Each column header has two lines: the column name, and under it its type in muted
  mono and smaller (`int4`, `timestamptz`, `varchar(30)`); the sort indicator sits at the
  right. Primary key columns get a small key mark before the name. (table-view.md T4)
- Clicking a header sorts **on the server** (`table.fetchPage` with `sort`) for table
  tabs, and on the client for query results (as today). The current sort column shows
  its direction. A third click clears the sort.
- Long values truncate with an ellipsis. Selecting a cell shows the accent frame, and
  its row lifts one surface step. Rows alternate (stripes).
- Query results show the gutter too, and show types when the driver reports
  `Column.type_name`.
- **Status:** built. The sort cycles descending, ascending, off. Row numbers are
  positional, so they stay 1, 2, 3… after a sort.

**Commands:** `table.fetchPage` (sort), `schema.describeTable` (PK and types for table
tabs). Engine already returns `Column.type_name`; drivers fill it.

## 5. Status bar for the table tab

**Why:** the current footer and status bar split the same facts across two lines.

**Behaviour**
- One bar at the bottom of the window. On the left: connection dot, connection name,
  driver name in muted text.
- In the middle, for a table tab: a *Structure / Data* switch, the row count and timing,
  and when the data was last fetched (`just now`, `2 min ago`), updating live.
- On the right: a page number box you can type into (Enter jumps, clamped to the page
  count), next / previous buttons, a *Refresh* button (refetch the current page), and an
  *Add row* button (todo until `table.stageChange`). Export / Copy as move into a menu
  button at the far right.
- For a query tab the middle shows the statement count, rows and timing (today's text).
- Error messages still replace the middle text in the danger colour.
- The bar is a neutral surface from the ramp. It is **not** coloured by the connection
  (the dot carries the connection colour, per `ui.md`).

**Commands:** `table.fetchPage` (page, refresh), `result.export`, `export.table`.

**Status:** built, with two changes from the plan above:
- The connection (dot, name, driver, read-only) stays in the slim header at the top instead
  of moving to this bar, so it shows in one place only.
- **Structure (SQL tables):** a row of tabs, *Columns · Indexes · Relations · Triggers ·
  DDL*, each with its count, and a reload button on the right. Each tab is one read-only
  list: a header row of muted labels over a bordered panel of 36px hairline rows. Columns:
  name, type (mono), nullable (a check), default (mono, `NULL` when none), comment,
  primary (key icon, plus the position when the key has several columns). Indexes: name,
  unique, primary, columns. Relations: name, columns, referenced `schema.table`,
  referenced columns, ON UPDATE, ON DELETE. Triggers: name, timing, event, action on one
  line, WHEN condition. DDL: the CREATE statement. Everything comes from one
  `describe_table` call that runs in the background when the tab opens; the DDL loads in
  the background the first time Structure or the DDL tab is shown. Until it arrives the
  list says `Loading structure…`. MongoDB collections keep their own structure view.
- **Changing structure:** on a writable connection, Columns, Indexes and Relations have a
  `plus` button next to reload and an `x` at the end of each row (not on the primary key's
  index, not on triggers). Column rows also have a `pencil`: the same form, filled with the
  column as it is, renames it and changes its type, NULL and default (the preview says
  "nothing changed" until a field differs). `plus` opens a form (Add column: name, type, default, nullable;
  Create index: name, columns in order, unique; Add relation: optional name, columns,
  `schema.table`, referenced columns, ON UPDATE / ON DELETE) whose *Will run* box shows the
  exact SQL as you type, or why it cannot run yet. `x` asks first with the statement and
  "cannot be undone" (dropping a column also says its data goes). Both run in the
  background, then the structure and DDL reload and the status bar says so. Read-only
  connections show neither button and the engine refuses the change
  (`structure.preview`, `structure.apply`).
- The Data / Structure switch stays at the top of the table tab. *Add row* waits for
  `table.stageChange`.
The bar shows `6 rows · 0 ms · fetched just now`, then any message. On the right: previous,
a page box, `of N`, next, refresh, and an *Export* menu (CSV / JSON file to
~/Downloads; CSV / JSON / Markdown / SQL INSERT to the clipboard). The old table footer is
gone. Query tabs keep their *Copy as* buttons above the results.

## 6. Tab strip

**Behaviour**
- A table tab's title is the table name followed by a muted hint of its filter state:
  nothing when unfiltered, `filtered` when a filter is applied.
- The *+* button for a new query sits right after the last tab.
- Middle-click closes a tab. Closing the active tab activates the one to its left.

**Commands:** `tab.new`, `tab.close`.

---

## Order of work

1. Filter row (3): engine is ready, biggest daily win.
2. Grid gutter and headers (4): needs server-side sort and PK info.
3. Status bar merge (5).
4. Schema-grouped lazy tree (2): needs `ObjectRef.schema` filled by drivers; matters once
   the PostgreSQL driver lands.
5. Connection / database picker (1): needs `schema.listDatabases` / `schema.setDatabase`.
6. Tab strip details (6).

## 9. Data transfer

**Why:** moving tables between servers (dev to staging, SQLite to PostgreSQL, a
collection into another cluster) should not need a dump, a file and a restore.

**Behaviour**
- Opened from a database's right-click menu (*Transfer data*, starting from that
  connection and database) or the command palette (starting from the connection in
  front). The dialog is ours: *From* and *To* side by side with a swap button between,
  each a connection picker (searchable; every row and the picked value show the
  connection's colour dot, its database's logo and its name) and a database
  picker; picking opens the dialog's own sessions in the background (`Connecting…`,
  `Opening database…` under the pickers) and closes them with the dialog.
- *Tables* lists the source database's tables (collections on MongoDB) with checkboxes,
  a filter box, a `2 of 14` count and *Select all* / *Clear*.
- *Copy*: Structure and rows · Structure only · Rows only (into tables that already
  exist). *When a table already exists*: Append rows · Empty it first · Drop and
  recreate.
- *Start transfer* (primary) runs off the UI thread; the footer reads
  `Copying users · 1000 of 1203 rows`, *Close* turns into *Stop*, and the result
  stays there (`Copied 2 tables · 1,203 rows`, or the database's error in red).
- Saved transfer tasks (a list beside the form) are todo.
- Tested against live servers: `crates/thaan-cli/tests/transfer_live.rs` copies a
  SQLite table to PostgreSQL, MySQL and MongoDB and back (`--include-ignored`).

**Commands:** `data.transfer`.

