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

**Commands:** `table.fetchPage` with `filters` (engine done). No new command.

## 4. Grid gutter and column headers

**Behaviour**
- A narrow gutter on the left of the grid shows the row number in muted mono, counted
  across pages (page 2 of 100 rows starts at 101). It stays fixed while scrolling
  horizontally and doesn't scroll away.
- Each column header shows the column name, then its type in muted mono and smaller
  (`int4`, `timestamptz`, `varchar(30)`), then the sort indicator. Primary key columns
  get a small key mark before the name.
- Clicking a header sorts **on the server** (`table.fetchPage` with `sort`) for table
  tabs, and on the client for query results (as today). The current sort column shows
  its direction. A third click clears the sort.
- Long values truncate with an ellipsis. Selecting a cell shows a 1px accent outline.
- Query results show the gutter too, and show types when the driver reports
  `Column.type_name`.

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
