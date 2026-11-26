# MCP server

`thaan mcp` lets an AI agent (Claude Code, Claude Desktop, Cursor, any MCP client) use
ThaanDB's engine: the same connections, drivers and commands as the desktop app. It speaks
the Model Context Protocol over stdio (one JSON-RPC message per line), written from the
public MCP specification. The protocol lives in `thaan_core::mcp`; `thaan-cli/src/mcp.rs`
is the stdio loop.

## Setup

### From the app you downloaded

The server ships inside the app, so there is nothing to build:

- **macOS:** `/Applications/ThaanDB.app/Contents/MacOS/thaan` (next to the app's own
  binary). The download is not code-signed, so macOS keeps a quarantine flag on it and
  may refuse to start `thaan` when an agent launches it. Clear the flag once after
  unzipping: `xattr -dr com.apple.quarantine /Applications/ThaanDB.app`.
- **Windows:** `thaan.exe`, in the same folder as `ThaanDB.exe`.

**Settings › MCP** in the app finds this binary and shows the exact command or config
text for Claude Code, Claude Desktop, Cursor, VS Code, Codex and Windsurf, with this
machine's paths filled in:

```bash
claude mcp add thaandb -- /Applications/ThaanDB.app/Contents/MacOS/thaan mcp
```

### From source

Build the CLI once (`cargo build --release -p thaan-cli`), then register it:

```bash
claude mcp add thaandb -- /path/to/target/release/thaan mcp
```

Claude Desktop / other clients (`mcpServers` in their config):

```json
{ "mcpServers": { "thaandb": { "command": "/path/to/thaan", "args": ["mcp"] } } }
```

It reads the same store as the app (saved connections, history) and, with the default
`keychain` feature, the passwords the app saved. `--store FILE` points it at another store.

## Tools

Every tool that needs a database takes `connection`: a saved connection's name or id, or
a URL (`sqlite:./app.db`, `postgres://user:pw@host/db`, `mysql://…`, `mongodb://…`,
`redis://…`). It is opened on first use and kept open, so there is no connect step and no
session id to pass around.

| Tool | What it does |
| --- | --- |
| `list_connections` | Saved connections and open sessions |
| `add_connection` | Save a connection from a URL (or a full config); tested first |
| `duplicate_connection` | Save a copy of a saved connection, password included (never shown) |
| `query` | Run statements: SQL, MongoDB shell syntax, Redis commands. `params` for placeholders, `maxRows` (default 200, max 5000) |
| `list_databases` | Databases on the server |
| `list_tables` | Tables, views, indexes, triggers / collections / keys |
| `get_schema_context` | Every table's columns (type, PK, NOT NULL, default, FK target) and the routines' names, in one call |
| `describe_table` | Columns, keys, indexes, triggers and the CREATE statement |
| `fetch_rows` | One page of a table with total count; `where`, `sort` (`["-age"]`), `page`, `pageSize` |
| `list_routines` | Stored functions and procedures (PostgreSQL, MySQL/MariaDB) |
| `get_routine_source` | A routine's CREATE FUNCTION / PROCEDURE |
| `open_table` | Open a table in the ThaanDB window (connects first if needed) |
| `execute_and_show` | Run statements in a new query tab of the ThaanDB window; read-only SQL also returns the rows |
| `disconnect` | Close a connection's session |
| `list_commands` | Every engine command id and its parameters |
| `run_command` | Run any command (structure changes, staged edits, rename / truncate / drop, data transfer, export, saved queries, history, settings); `connection` fills in `sessionId` |

## Passwords

An agent never sees a connection password. The server sends every reply and error
through one redaction (`thaan_core::mcp::redact`) before it leaves:

- fields named like a secret (`password`, `sshPassphrase`, `token`, …) read `••••••`;
- in every string, the passwords this process holds (saved connections, open sessions)
  are masked, and so are URL passwords (`postgres://u:••••••@h`) and `password=…`,
  `PASSWORD '…'`, `IDENTIFIED BY '…'`;
- when a connect fails, the passwords read from the keychain for it are masked in the
  driver's message too.

`run_command connection.list` is covered like everything else. Query rows are the
database's own data and pass unchanged. Agents can still use saved passwords (connect,
duplicate) without seeing them; the keychain entry stays where the app put it.

## Showing things in the app

`open_table` and `execute_and_show` need the desktop app running. At startup the app
listens on 127.0.0.1 (a free port) and writes `app-link.json` next to the store
(readable by your user only) with the port and a random token; `thaan mcp` sends one
JSON line with that token per request. The app shows saved connections only, connects
when needed and brings its window forward. Requests that arrive while a connection is
still opening wait for it (up to 20) instead of failing, so an agent can call
`open_table` and `execute_and_show` back to back; a request for another connection
opens that one once the first is done. `execute_and_show` runs anything that may
write only in the app (never twice); read-only SQL also runs in the server so the agent
gets the rows.

## Permissions

There is nothing to set up: the agent can do everything the app can. The only guard is the
connection's own **Read-only** flag, enforced by the engine (the same as in the app), so mark
production connections read-only if an agent should not write to them. Failures come back
as tool results with `isError: true` and the database's message, so the agent can fix the
statement and retry.

## Limits

- Replies are capped (rows per statement, 2,000 characters per text cell) to keep the
  agent's context small. The engine still loads the full result of a query first (known
  gap 4 in `CLAUDE.md`); add `LIMIT` for large tables or use `fetch_rows`.
- Each request runs on its own thread, so a slow query does not block other calls.
