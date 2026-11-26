# Table view spec (observed workflow)

What a table tab should offer once it is open: the bars above the grid, the grid itself,
and the bar below it. Plus the key browser that Redis needs instead of a grid.

**Source type:** observed behaviour. On 2026-11-26 we used another desktop database
client the normal way: we opened two PostgreSQL tables and one Redis database on the
maintainer's dev connections and took screenshots of its window. Nothing was read from
that client's source, bundle or binaries (see `docs/clean-room.md`). We take the
*workflow and the order of controls* only. Colours, icons, wording, sizes and pixel
layout stay ours (`docs/spec/ui.md`). The screenshots are not kept in the repo, because
they show the maintainer's data.

Builds on `workspace.md` §3–§6 (filter row, gutter and headers, status bar, tab strip),
which are already built. Each item below says what is **new** compared with today.

---

## T1. Tab strip

**Observed:** every tab starts with an icon for what it holds (a table, a key-value
database), then the name, then a close button. The active tab is set apart by a tinted
surface and a thin accent edge.

**For us**
- Table tabs get the `table` icon, query tabs `file-code`, a Redis database or MQTT topic
  tab its driver logo (`db_icon`). Icons take the text colour, as everywhere.
- *New:* the icon. The close button and the active state exist already.

## T2. Location bar

**Observed:** one thin row under the tabs reads like a path: connection › schema and
database › the table name (set apart as a small chip) › a muted column count. The right
end of the same row holds the actions about the table rather than its rows: edit the
structure, choose which columns show, jump to a column, table information, and a menu
of data tools.

**For us**
- A breadcrumb row: `connection › schema · database › table` and `7 columns` in muted
  text. Clicking a part selects it in the sidebar tree.
- On the right, ghost buttons: *Structure* (the existing Data / Structure switch moves
  here), *Columns* (a checklist to hide and show columns, kept per table for the session,
  no refetch, the grid just skips them), *Go to column* (a small search that scrolls the
  grid to that column and outlines its header), *Info* (row estimate, size on disk,
  owner, comment, as far as the driver reports them).
- *New:* the whole row. Hidden columns and *Go to column* are grid-only work. *Info*
  needs a driver hook (`schema.tableInfo`, *new* command) and can come later.

## T3. Query bar

**Observed:** one row of controls that shape what the grid shows and what is pending:
- a scope picker on the far left (*All rows*);
- a `WHERE` box and an `ORDER BY` box side by side, each a one-line code field with its
  keyword shown as the placeholder and a small menu of recent entries;
- a button that copies the query the grid is showing;
- *Refresh*, and next to it an *Auto* toggle that refreshes on a timer;
- *Add row* with a small menu beside it (more ways to add), *Delete row* (lit only while
  a row is selected), *Commit* and *Rollback* (lit only while edits are staged).

**For us**
- Keep our filter builder and its raw `WHERE` toggle (workspace §3). In raw mode, show a
  second box for `ORDER BY` next to it. It overrides header sorting while it has text, and
  clicking a header clears it. *New:* the ORDER BY box (`PageRequest.raw_sort` already
  exists for MongoDB; SQL drivers need `table::page_queries` to use it, bound the same
  way the raw WHERE is: as typed, inside the dialect's paging).
- Each box keeps its last 10 entries for the table, in memory. *New.*
- *Copy query* puts the SQL of the current view on the clipboard. We already build it
  (`view_as_query`); this only adds the button.
- *Auto refresh*: off, 5 s, 30 s, 1 min. While on, the grid keeps the old page visible
  and swaps in the new one when it arrives (principle 9). It pauses while edits are staged
  and when the tab is not visible. *New.*
- The edit actions (*Add row*, *Delete row*, *Apply*, *Discard*) move from the status bar
  into this row, so everything that changes data sits together above the grid. The
  status bar keeps counts and paging. Labels stay ours: *Apply* and *Discard*.
- Redis and MQTT tabs show the same row, with the builder only (no raw `WHERE`).

## T4. Grid

**Observed:**
- A `#` gutter on the left.
- A two-line header for each column: an icon before the name (a key for the primary key,
  a different mark for indexed and foreign-key columns), the name on the first line and
  the declared type on a second, smaller line, with the type's colour depending on its
  family. On the right of each header, a sort toggle and a small menu.
- Alternate rows are a step lighter. Cell text is monospaced.
- A selected cell gets an outline, and its whole row a faint tint.

**For us**
- Header on two lines: name, then the type under it in muted mono. *New:* today the type
  sits on the same line. We keep the type in one muted colour. Colouring by family would
  add a palette for little gain (`ui.md`: colour only means state).
- Marks before the name: `key-round` for the primary key (exists); *new* `link` for a
  foreign-key column and `list` for an indexed one, from the `describe_table` result the
  tab already loads.
- A header menu (`chevron-down`): Sort ascending, Sort descending, Clear sort, Filter by
  this column (adds a builder row), Hide column, Copy column name. *New.*
- Zebra rows: a one-step surface shift from the neutral ramp. Already built (the kit's
  stripe).
- Selected row tint plus the existing cell outline. *New:* the row tint.

## T5. Status bar

**Observed:** left to right: `Total N rows` with the exact count as a link, the column
count, the time the fetch took, the SQL that produced the grid (with a copy button);
then on the right a sum of the selected cells (with a menu for other aggregates), the
page count, a page-size picker, first / previous / page box / next / last, a button that
loads all the rest, and an *Export* menu.

**For us**
- Keep today's order and add:
  - the statement text, muted mono, cut with an ellipsis, full text on hover, copy
    button. *New.*
  - *Selection summary*: when two or more cells are selected, `Sum · Avg · Min · Max ·
    Count` of the numeric ones, one shown at a time, picked from a menu. Computed from the
    page already in memory. *New.*
  - A page-size picker: 50 (default), 100, 500, 1000. *New.* The default stays 50
    (principle 9). The loaded page is still the only copy in memory.
  - First and last page buttons beside previous and next. *New.*
- We do **not** add "load everything": it breaks the paging and memory rules.
- The exact count is what we already show. If a driver can only estimate, it says
  `≈ N rows` and the number becomes a button that runs the exact `COUNT(*)` in the
  background.

## T6. Redis key browser

**Status: built** (`key_browser.rs`, `keys.scan`), reworked on 2026-11-26 from two more
screenshots the maintainer shared of the other client (key tree with folders and badges;
a list key's detail with an add row). Built: folders by `:` with counts, type and TTL
badges (our theme's base hues, our own pairing), checkboxes, *Select all*, bulk delete,
*Fuzzy*, *No expiry*, `+` *Add key*, `N / M keys loaded`, and on the right the key header
with badges, an item search and an add row. A third round of screenshots (the key header's
actions, a rename dialog, an item viewer with decode and view choices and Edit / Copy)
added *Rename key* and the item dialog: View as Text / JSON / Hex / Base64, word wrap,
Copy, Edit and Save. Then the decoders (GZip, Zlib, Deflate, Base64,
MessagePack, Pickle, Protobuf without a schema), *Set expiry* from the header, and a
*Command line* tab. Not yet: searching values, custom grouping rules, PHP and Java
serialised decoders, plug-in decoders, and the Pub/Sub and Slow log tabs.

**Observed:** a Redis database opens as one tab split in two panes.
- Left pane: a switch for what the search box matches (key name, value, or both), the key
  count, refresh, and *add key*. Under it a pattern box (`user:*`) with recent entries,
  a *fuzzy* toggle, a *no expiry only* toggle. Then a key-type picker (*All key types*)
  and controls to group keys into a tree by a separator. Then the key list, with an empty
  state when nothing matches.
- Right pane: tabs *Key detail · Command line · Pub/Sub · Slow log*, with
  `Select a key to view details` until a key is picked.

**For us** (replaces "every key in the sidebar" from `features.md` §9a)
- Opening a Redis database (or `db0` in the tree) opens a **key browser tab** instead of
  listing keys in the sidebar. The sidebar shows only `db0…dbN` with their key counts
  (`INFO keyspace`).
- Left pane: pattern box (glob, sent as `SCAN MATCH`), a type picker (`SCAN … TYPE`), a
  *no expiry* toggle (filters on `TTL = -1` for the loaded batch), a *group by `:`*
  toggle that folds keys into a tree. Keys load in batches of 200 with *Load more* at the
  bottom; the list is virtualized. Each row: type badge (text, not colour), key, TTL,
  size when cheap. *Add key*: name, type, first value, TTL.
- Right pane, *Key* tab: the per-type grid we already have (`fetch_page` on that key),
  with TTL and type above it, *Rename*, *Set TTL* and *Delete* actions.
- *Console* tab: the line-based command editor we already have, bound to this database.
- *Pub/Sub* and *Slow log* tabs: later (`SLOWLOG GET`; a Pub/Sub listener needs its own
  connection, like the MQTT reader thread).
- Engine: `redis.scanKeys` (*new* command: `{sessionId, pattern?, type?, cursor, count}`
  → `{keys: [{name, type, ttl}], cursor}`), so the GUI pages with a cursor instead of an
  offset.

## T7. MQTT console

**Observed** (a screenshot of the other client, shared by the maintainer on 2026-11-26):
a broker connection has one child, a console. It shows the broker address and its state;
on the left the saved subscriptions with a search, a *new subscription* action and a quick
subscribe box at the bottom; on the right the messages of all topics with an
All / Received / Sent switch, a payload search, a payload format picker, Pause, and
expand / collapse of payloads; under them a publish panel: topic, QoS 0/1/2, retain,
payload encoding, payload, and a publish button (Ctrl+Enter).

**Built** (`broker_console.rs`; reworked on 2026-11-26 after the maintainer asked for an
easier console):
- The tree shows one *Console* row under an MQTT connection (no databases, no folders).
- Left: *Subscriptions* with a count, each row a filter in mono, its granted QoS and an
  unsubscribe `x`; at the bottom a filter box, QoS 0 / QoS 1, *Subscribe* (Enter works).
- Right: All / Received / Sent, *Search payloads* (topic too), the count, Pause/Resume, and
  Clear (only the list; the broker is not touched). Rows: direction arrow, topic, local
  time, `retained`, QoS, size, and the payload on one line. Clicking a row shows the
  payload in full under the list, with a copy button.
- Publish panel (hidden on read-only connections): topic, payload editor, QoS 0 / QoS 1,
  Retain, *Publish*. The outcome shows next to the button.
- Polls `broker.messages` every 500 ms with the last id seen; keeps 2,000 messages.

Not yet: QoS 2 (the driver tops out at QoS 1), payload formats (JSON pretty, hex,
base64), saved subscriptions across reconnects, ⌘↵ to publish, and a topic tree.

---

## Order of work

1. T4 two-line header, zebra rows, row tint (grid only). **Built** (stripes were
   already on; FK / index marks and the header menu are still to do).
2. T5 statement text, page-size picker, first/last, selection summary.
3. T3 ORDER BY box, copy query, move edit actions up; then auto refresh.
4. T2 location bar with Columns and Go to column; Info later.
5. T6 Redis key browser (engine command first, then the tab). **Built**, and T7 the
   MQTT console with it.
6. T1 tab icons.

Each step goes through `/unslop` first and updates `ui.md` and `features.md` as it lands.
