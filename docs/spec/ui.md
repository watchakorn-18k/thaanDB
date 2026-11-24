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

The palette lives in `crates/dbm-app/src/theme.rs` as named tokens (`bg`, `panel`, `border`,
`text`, `muted`, `accent`, `danger`, `success`, `warning`). The accent is a teal (#14b8a6
in dark mode), a deliberate choice of our own.

## Data-driven UI

- Connection forms are generated from `DriverInfo::params` (a parameter schema).
- Menus, the command palette and shortcuts read `CommandRegistry`; the GUI never hardcodes
  an action that is not a registered command.
