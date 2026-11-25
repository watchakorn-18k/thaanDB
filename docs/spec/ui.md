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

- Surfaces stay graphite; the teal marks only the primary action, the active tab and the caret.
- The connection's own colour appears only as an 8px dot (title bar, saved list). It is the
  one other colour in the chrome.
- Labels are sentence case at weight 500, never uppercase; counts follow in muted mono
  (`Tables 2`). No italic: `NULL` is muted mono text.
- Figures (numeric cells, counts, timings, page numbers) and SQL use the mono face; numeric
  cells align right.
- Spacing is on a 4px grid. One-line rows in the sidebar are 28px, palette rows 32px,
  toolbars and the header 36px.
- Shortcuts are drawn as keycaps. Copy says `1 row` / `6 rows`, never `row(s)`, and short
  labels and empty states end without a period.
- Table tabs keep one bottom status bar; its *Export* menu holds the file exports and the
  clipboard copies (workspace.md §5).
- Command palette: commands grouped under their category label, shortcuts as `Kbd`, ↑ ↓ to
  move, ↵ to run, Esc to close. An empty search lists only what this window can run right
  now (without a connection, just the theme toggle); typing also finds CLI-only commands,
  shown muted with a `CLI` tag. Nothing destructive is preselected.

## Data-driven UI

- Connection forms are generated from `DriverInfo::params` (a parameter schema).
- Menus, the command palette and shortcuts read `CommandRegistry`; the GUI never hardcodes
  an action that is not a registered command.

## Title bar

- No native title bar. The window uses gpui-kit's `TitleBar` (34 px): on macOS the traffic
  lights sit at its left, then the current context (the ThaanDB mark and name on the home
  screen, or the open connection's dot, database logo, name and driver), a hairline
  divider, a *New connection* button, and *Commands* with its `Kbd` on the right.
- The empty parts of the bar drag the window; a double click zooms it like a native bar.
- On Windows (and Linux with client-side decorations) the same `TitleBar` draws the standard
  minimise, maximise/restore and close buttons at the right end, after *Commands*. They use
  the system's own handling (Snap Layouts on maximise, red close on hover). macOS shows the
  traffic lights on the left instead. Do not draw window buttons by hand.
