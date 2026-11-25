# Home screen and New connection dialog

The Home tab: the first tab of the one-page window (workspace.md §1a), shown whenever
it is selected or no other tab is open, connected or not. Written in our own words.

> The window no longer has a separate home page with its own sidebar: the workspace
> sidebar tree lists the saved connections (with the menu below) in every state. The
> sidebar notes below describe the old home sidebar and are kept for its behaviour
> (groups, menu, drag and drop), now partly carried by the tree; drag and drop in the
> tree is todo.

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
  - **Then connections in no group.** Each row: database logo, name, `Connecting…` while it
    connects, a green dot and a → button once it is open, delete. Clicking a row (once or
    twice) only connects and stays on home; it never opens the workspace. The → button (or
    right-click → Open) opens it. An open connection also has a ›/⌄ arrow before its logo
    that unfolds the same tree as the workspace sidebar (databases → Tables / Views →
    objects, loaded lazily); clicking a table there opens it in the workspace. Quick connect works the same, with an *Open* button on
    open connections; the dialog's *Connect* also stays on home when started from home.
  - **Right-click a connection:** *Connect* (connects in the background and stays on home),
    *Connect and open* (or *Open* and *Disconnect* when it is already open; both switch
    this window to the connection's page, never a new window), *Edit* (the
    form filled in; Save updates the same entry), Duplicate (saved as "… copy" right below), Copy ▸
    (Name; Connection URL, e.g. `postgres://app@host:5432/shop?sslmode=require`, without the
    password; Connection URL with password, read from the keychain off the UI thread and
    percent-encoded; File or host), Move to group ▸ (every group, its current one greyed, then
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
- **What the Home tab shows** (redesigned 2026-11-26: only what gets used; the stat
  cards, Quick connect, Shortcuts card and From the terminal card are gone):
  - **Connections** with their count and a *New connection* button. One row per saved
    connection, in saved order: the database logo in a small tile, the name, its colour
    dot and its group name (muted), then driver · target (target in mono). On the right,
    when it last ran a query (`19 h ago`, from history) or a green dot and `open` once it
    is connected, then one button: *Connect* (stays on home), *Connecting…* with a
    spinner while it connects (every Connect is disabled meanwhile), or *Open* once
    connected (a query tab for it). A double click on the row connects; right-click is
    the same menu as the sidebar. With none saved: `No saved connections` and a line on
    what adding one gives.
  - **Recent queries:** up to eight, newest first, only from connections that are still
    saved, the same statement on the same connection shown once. Each row: a red dot for
    a failure, the first line in mono, the connection name, rows · ms (or `failed` in the
    danger colour) and how long ago. Clicking opens it in a new query tab of its
    connection, connecting first when needed. Hidden when empty.
  - **Key hints:** one quiet line of keycaps: Commands, New query tab, Run statement.
- **Back to home:** in the workspace the *Home* tab returns here; open connections stay
  open, with their tabs.
- **Cached, not asked in render:** the recent list and last-used times are read from the
  store at startup, after each query and when history is cleared.

## New connection dialog

A modal over the window, closed by its *×*, by clicking outside, or by finishing.

1. **Pick a database type.** On top, a *Tiles / List* switch (`layout-grid` /
   `layout-list`) and a search box. Below, a 184px column of families (*All* first, then
   Embedded, Relational, Document, Key-value, Message broker…, each with its count; the
   picked one is a surface step) and the types on the right: under *All* grouped by
   family with a heading, under one family without one. Tiles are 148×112 with the logo
   and name; list rows are 52px with the logo, name and a one-line note. Search filters
   within the picked family. One type is selected (accent border); the footer says which.
   *Next* (or a double click) goes to step 2. The family and view are view state only.
2. **Fill in the fields.** Name, connection URL, the driver's own fields (from
   `DriverInfo::params`), read-only. A back arrow returns to step 1. The footer has *Test*,
   *Save* (saves and closes) and *Connect* (closes and connects).

Commands: `connection.list`, `connection.save`, `connection.delete`, `connection.test`,
`connection.open`, `history.list`, `savedQuery.list`.
