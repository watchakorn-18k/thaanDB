# Home screen and New connection dialog

What the window shows while no session is open. Written in our own words.

**Source type:** observed behaviour, from screenshots of another desktop database client's
start page and connection dialog, shared by the maintainer on 2026-11-25. Only the workflow
is taken; layout details, wording, icons and colours are ours (`docs/clean-room.md`,
`docs/spec/ui.md`).

## Home

- **Sidebar (280px):** a search box that filters saved connections by name, a *+* button for a
  new connection, then every saved connection (driver icon, name, colour dot, delete).
  Clicking a row connects.
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
