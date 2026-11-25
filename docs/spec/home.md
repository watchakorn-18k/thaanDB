# Home screen and New connection dialog

What the window shows while no session is open. Written in our own words.

**Source type:** observed behaviour, from screenshots of another desktop database client's
start page and connection dialog, shared by the maintainer on 2026-11-25. Only the workflow
is taken; layout details, wording, icons and colours are ours (`docs/clean-room.md`,
`docs/spec/ui.md`).

## Home

- **Sidebar (280px):** a search box that filters saved connections by name, a *New group*
  button (`folder-plus`) and a *+* button for a new connection, then the connections:
  - **Groups first,** in the order they were made. A group row shows a chevron, a folder
    icon, its name, its connection count in muted mono and a delete button. Clicking the row
    folds or unfolds it (view state only). Its connections sit under it, indented.
  - **Then connections in no group.** Each row: database logo, name, colour dot, delete.
    Clicking a row connects.
  - **Right-click a connection:** Connect, Duplicate (saved as "… copy" right below), Copy ▸
    (name, file or host), Move to group ▸ (every group, its current one greyed, then
    *No group*), New group…, and *Delete connection…* in the danger colour (asks first).
  - **Right-click a group:** New group…, *Delete group…* (asks first).
  - **Drag a connection** (press, hold, move): drop it on another connection to place it
    just above that one, in that connection's group (a 2px accent line marks the spot);
    on a group header to move it to the end of that group (the header highlights and the
    group unfolds); on the empty space below the list to take it out of any group. The
    order is saved (`connection.moveToFolder` with `before`).
  - A search shows groups whose name or connections match, unfolded.
- **New group:** a small dialog with a name box (Enter or *Create*). Names are unique,
  ignoring case. An empty group stays until it is deleted.
- **Delete group:** asks first (`Delete group "Production"?`) and says the connections stay;
  they move out of the group, none is deleted.
- **New connection form:** a *Group* picker (*No group* first) sets the group on *Save*.
- **Counts:** four compact cards: saved connections, database types (registered drivers),
  saved queries, queries run (history). Numbers only.
- **Quick connect:** the first five saved connections with driver name and target (file
  path, or host:port/database). Clicking connects. With none saved, a *New connection*
  button.
- **Shortcuts:** New connection, Command palette (with keycaps), light or dark theme, plus a
  one-line hint.
- **Recent queries:** the last five history entries (SQL in mono, rows and time, a red dot
  for a failure).
- **From the terminal:** a note that every action is a `thaan` CLI command, with two
  examples. (No MCP server yet, so it isn't mentioned.)

## New connection dialog

A modal over the window, closed by its *×*, by clicking outside, or by finishing.

1. **Pick a database type.** A search box and a grid of tiles grouped by family (Relational,
   Embedded, Document). One tile is selected; the footer says which. *Next* (or a double
   click on a tile) goes to step 2.
2. **Fill in the fields.** Name, connection URL, the driver's own fields (from
   `DriverInfo::params`), read-only. A back arrow returns to step 1. The footer has *Test*,
   *Save* (saves and closes) and *Connect* (closes and connects).

Commands: `connection.list`, `connection.save`, `connection.delete`, `connection.test`,
`connection.open`, `history.list`, `savedQuery.list`.
