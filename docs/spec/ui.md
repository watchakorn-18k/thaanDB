# UI spec

Our own design. It follows the common SQL-client layout so the app feels familiar, but
colours, icons and spacing are ours (see `docs/clean-room.md`).

```
┌──────────────────────────────────────────────────────────────────────────┐
│ title bar: connection name · database · read-only badge     ⌘K palette   │
├───────┬───────────────────┬──────────────────────────────────────────────┤
│ rail  │ sidebar           │ tab strip  [Query 1] [users] [+]             │
│       │ ┌ connections ┐   ├──────────────────────────────────────────────┤
│ conn  │ │ ● local.db  │   │ editor (query tab) / filter bar (table tab)  │
│ saved │ └─────────────┘   │                                              │
│ hist  │ filter…           ├──────────── drag to resize ──────────────────┤
│       │ ▸ tables          │ results grid                                 │
│       │   ▸ users         │                                              │
│       │ ▸ views           ├──────────────────────────────────────────────┤
│       │                   │ status: 42 rows · 3 ms · page 1/5 · Apply(2) │
└───────┴───────────────────┴──────────────────────────────────────────────┘
```

- **Rail**: icons for Connections, Saved queries, History.
- **Sidebar**: content of the selected rail item; object tree with filter.
- **Tabs**: query tabs and table tabs; a table tab has sub-views *Data* and *Structure*.
- **Editor / results**: vertical split, resizable.
- **Status bar**: per-tab facts and staged-change actions.

## Theme tokens

The palette lives in `crates/thaan-app/src/theme.rs` as named tokens (`bg`, `panel`, `border`,
`text`, `muted`, `accent`, `danger`, `success`, `warning`). The accent is a teal (#14b8a6
in dark mode), a deliberate choice of our own. Text on the dark-mode teal is near-black
(#06201d) for contrast; the light-mode teal (#0f766e) takes white.

## Visual rules

- Surfaces stay graphite; the teal marks only the primary action, the active tab, the caret
  and the grid's selection.
- A row staged for deletion is solid red (danger at 50% over the background, painted on
  its cells) and stays that colour on hover; staged-edit and added rows keep their own
  marks.
- Grid selection: the selected cell gets a 1.5px teal frame over the whole cell with 4px
  corners and no fill, and a 6px round teal handle in its bottom-right corner, like a
  spreadsheet's fill handle. The grid draws the frame itself; the kit's square frame is
  turned off in `theme.rs` (`table_active_border` transparent). Its column header lifts one
  surface step (foreground at 6%), and so does its row: a solid step (foreground at 6%
  over the background) that replaces the stripe under it, never the teal. No filled
  header, no filled row number, no column wash.
- Grid headers are 40px and hold two lines: the name (medium weight, after the key mark
  for primary-key columns), then the declared type under it in muted mono at 12/16.
  Rows alternate one surface step (the kit's stripe). While a cell is being edited the teal frame is the
  editor's only border: the text box has no box, border, radius or inset of its own and
  starts where the cell's text starts, the cell lifts one surface step (foreground at
  6%), and the corner square is hidden. Numbers keep the mono face while being typed. Hovering a cell shows its full value in a
  tooltip (capped at 2,000 characters). Double-clicking a column's resize handle fits the
  column to its header and the cells on the page (at most 640px).
- Table and collection tabs open on the data with the query box collapsed; the *Query*
  button opens it.
- The connection's own colour, picked freely when the connection is made, is the one other
  colour in the chrome. It is used solid, never as a wash: it fills the bottom status bar while
  that connection is open, with black or white text chosen for contrast. Elsewhere (the Home
  connection list) it is an 8px dot after the name. The rows themselves stay neutral.
- In the workspace sidebar tree an open connection's logo, and its current database's
  icon, take the connection's colour (teal when it has none), followed by a 6px dot in
  the theme's `success` green that means "connected". Closed connections and other
  databases keep muted icons. The connection in front has the selected row background.
- Menu items never end in "…". Destructive items (Delete, Truncate, Drop) use the
  danger colour for the label and icon, through the kit's own icon column so they line
  up with the other rows.
- A sidebar row whose right-click menu is open keeps the row hover background
  (`sidebar_accent`) until the menu closes, so it is clear what the menu acts on.
- Labels are sentence case at weight 500, never uppercase; counts follow in muted mono
  (`Tables 2`). No italic: `NULL` is muted mono text.
- Figures (numeric cells, counts, timings, page numbers) and SQL use the mono face; numeric
  cells align right.
- Spacing is on a 4px grid. One-line rows in the sidebar are 28px, palette rows 32px,
  toolbars and the header 36px.
- Shortcuts are drawn as keycaps. Copy says `1 row` / `6 rows`, never `row(s)`, and short
  labels and empty states end without a period.
- One bottom status bar, 36px, in the connection's colour, in three parts that line up
  with the window above: (1) a block as wide as the sidebar (it follows the sidebar when
  resized) with the database logo in ink, the database the session is on (medium) and the
  driver name right-aligned at 75% ink; (2) for a table tab, a *Structure* / *Data* chip
  (12% ink well) that switches the view, then `hash` + row count with timing and `clock`
  + how long ago the page arrived, both mono, then any status message; (3) for the data
  view, the page box and arrows, a hairline, and three square 28×24 buttons on a 12%
  ink fill: refresh, add row (`plus`, *Add row* / *Insert document*, ⌘N; hidden on
  read-only connections) and `settings`, a menu with *Export whole table ▸* (CSV, JSON
  file), *Copy this page ▸* (CSV, JSON, Markdown, SQL INSERT) and *Copy view as SQL*
  (the current filters, sort and page as a SELECT with values inlined; *Copy view as
  find()* on MongoDB). Staged edits add their count, Discard and Apply after them.
  An added row shows `+` in the gutter instead of a number. Before the page box sits
  *Rows* (75% ink) and one 24px well: a typeable mono number (36px) and a small
  `chevron-down` that opens a narrow list (80px, no heading) of 1, 5, 10, 25, 50, 100,
  200 in mono, the current one ticked on the right. While the rows box or the page box has the
  cursor, its well gets a 1px ink ring and a 20% fill (a reserved transparent border
  otherwise, so nothing shifts).
- Under the filter of a table's data view sits one 36px action bar, the same on every
  database: ghost small icon buttons `plus` · `eye` · `pencil` | `trash`, then the
  selection in muted mono on the right (`_id …` on MongoDB, `Row 34` elsewhere).
- Command palette: commands grouped under their category label, shortcuts as `Kbd`, ↑ ↓ to
  move, ↵ to run, Esc to close. An empty search lists only what this window can run right
  now (without a connection, just the theme toggle); typing also finds CLI-only commands,
  shown muted with a `CLI` tag. Nothing destructive is preselected.

- A row staged for deletion in a data grid is filled with `danger` at 50% across the whole
  row until Apply or Reset; its text keeps the normal `foreground` ink so it stays readable.

## Data-driven UI

- Connection forms are generated from `DriverInfo::params` (a parameter schema).
- Menus, the command palette and shortcuts read `CommandRegistry`; the GUI never hardcodes
  an action that is not a registered command.

## Title bar

- No native title bar. The window uses gpui-kit's `TitleBar` (34 px): on macOS the traffic
  lights sit at its left, then always the ThaanDB mark and name (plus a `lock` *Read-only*
  note while a read-only connection is in front; the connection itself is named by its
  tab, sidebar row and status bar), a hairline
  divider, a *New connection* button, and *Commands* with its `Kbd` on the right, then a
  ghost `settings` icon button (tooltip *Settings ⌘,*).

## Settings

- A dialog opened by the title bar gear, ⌘, or the palette (`app.settings`), 720 px wide
  or the window less 16 px a side when that is narrower. On the left, 148 px of ghost
  page buttons: *General* (`sliders-horizontal`) and *MCP* (`bot`), the current one
  selected. Below 600 px of dialog the pages become a small segmented `TabBar` above the
  page. Both pages keep at least 420 px of height so switching does not make it jump.
- *General*: two sections in sentence case, *Appearance* and *Data*, split by a hairline.
  Each row is a label with a muted one-line hint on the left and a small outline
  `ButtonGroup` on the right, the current choice selected; when the row is too narrow
  the group wraps under the label (labels keep 180 px):
  - Theme: System · Light · Dark (System follows the OS and switches with it).
  - Text size: Small · Default · Large · Larger (15–18 px rem; default 16).
  - Rows per page: 25 · 50 · 100 · 200, for tables opened next (the status bar box
    still takes any size).
  - Query history: 100 · 500 · 1,000 · 5,000 entries kept.
- *MCP*: *MCP server* with a muted line saying agents use the saved connections through
  `thaan mcp`, can do anything the app can, and that Read-only keeps them from writing.
  *Server* with a status on the right (6 px dot + muted text: `Ready` in success green,
  `Not built yet` in warning, `Looking for thaan` while the background lookup runs),
  then a mono well (muted fill, 8 px radius, 12 px padding) with the binary path and the
  kit's `Clipboard` button; when there is none, the `cargo build` line in the same well.
  A hairline, *Add to a client*, a small segmented `TabBar` (Claude Code, Claude
  Desktop, Cursor, VS Code, Codex, Windsurf; it scrolls and gets an overflow menu when
  narrow), a muted line saying where the text goes, and the config in a well with
  `Clipboard`. Last line: `Connections from <store path>` in muted mono. When the app
  runs with `THAAN_HOME`, every config passes it on so agents see the same connections.
  The lookup (a `thaan` release build beside the app's binary, then the same folder,
  then `PATH`) runs off the UI thread each time the dialog opens.
- Changes apply at once; there is no Save button. They are written to the store off the
  UI thread, and a failed save says so in the status bar.
- General's last line: `Saved in <store path>` in muted mono, so people know where it lives.
- The rail's theme button and *Toggle theme* flip light/dark and save that choice too.
- The empty parts of the bar drag the window; a double click zooms it like a native bar.
- On Windows (and Linux with client-side decorations) the same `TitleBar` draws the standard
  minimise, maximise/restore and close buttons at the right end, after *Commands*. They use
  the system's own handling (Snap Layouts on maximise, red close on hover). macOS shows the
  traffic lights on the left instead. Do not draw window buttons by hand.

## Home tab

- Two sections in a 960px column (32px page padding, 32px between sections): Connections
  and Recent queries, each a 16px medium title (count in muted mono) above one bordered
  panel (radius 12, `sidebar` fill, hairline rows). No stat cards and no icon-headed cards.
- Connection rows are 60px: a 32px logo tile (radius 8, `background` fill, hairline), name
  and dot over driver · target, a 96px right-aligned last-used column in muted mono, and a
  112px action column (*Connect* ghost, *Open* outline). Recent rows are 40px.
- The only colours are the connection dots, the green `open` dot, the danger dot and
  `failed` text, and the one primary *New connection* button.
- Narrow windows: the column shrinks with the space it gets and never runs off the edge
  (names and targets truncate). When Home has less than 640px (window less rail and
  sidebar) it goes compact: 16px side padding and 24px on top, no last-used column (an
  open connection keeps its green dot before *Open*), no group name after the dot, the
  action column only as wide as its button, recent rows show just the query and how long
  ago, and the shortcut hints wrap.

## Key browser and broker console

- Both are two panes: a fixed list pane on the left (key browser 320px, console 280px)
  with a hairline on its right, and the content on the right. No folders in the tree for
  them: a Redis database row and an MQTT *Console* row open the tab directly; the tab
  icon is `key-round` / `radio-tower`.
- List rows are 28px (keys) and 48px (messages, two lines) and span the pane. The picked
  row lifts one surface step (foreground at 6%), hover 4%. Types, TTLs, times, QoS and
  sizes are 12px mono muted; key names and topics are the text colour.
- Section titles are sentence case at the body size, medium weight (*Subscriptions*,
  *Publish*); counts beside them in muted mono. Teal only on *Publish*.
- Waiting says so in words: `Scanning keys…`, `Loading 100 rows…`, `Waiting for
  messages…`, `Publishing…`.

