# MCP server

`thaan mcp` lets an AI agent (Claude Code, Claude Desktop, Cursor, any MCP client) use
ThaanDB's engine: the same connections, drivers and commands as the desktop app. It speaks
the Model Context Protocol over stdio (one JSON-RPC message per line), written from the
public MCP specification. The protocol lives in `thaan_core::mcp`; `thaan-cli/src/mcp.rs`
is the stdio loop.

## Setup

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
| `query` | Run statements: SQL, MongoDB shell syntax, Redis commands. `params` for placeholders, `maxRows` (default 200, max 5000) |
| `list_databases` | Databases on the server |
| `list_tables` | Tables, views, indexes, triggers / collections / keys |
| `describe_table` | Columns, keys, indexes, triggers and the CREATE statement |
| `fetch_rows` | One page of a table with total count; `where`, `sort` (`["-age"]`), `page`, `pageSize` |
| `disconnect` | Close a connection's session |
| `list_commands` | Every engine command id and its parameters |
| `run_command` | Run any command (structure changes, staged edits, rename / truncate / drop, data transfer, export, saved queries, history, settings); `connection` fills in `sessionId` |

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
