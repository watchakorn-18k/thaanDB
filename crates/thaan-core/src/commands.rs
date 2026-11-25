//! Command registry. Every user-visible action has a stable [`CommandId`] and JSON
//! parameters; GUI, CLI, MCP and plugins all go through [`dispatch`].
//!
//! Never rename an id. Add a new one and keep the old as an alias if behaviour changes.

use crate::connection::ConnectionConfig;
use crate::engine::{Engine, QueryRequest, SessionId};
use crate::export::ExportOptions;
use crate::schema::ObjectRef;
use crate::store::SavedQuery;
use crate::table::{PageRequest, StagedChange};
use crate::{Error, Result, ResultSet};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

pub type CommandId = &'static str;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    pub id: CommandId,
    pub title: &'static str,
    pub category: &'static str,
    /// Short description of the JSON parameters, shown by `thaan commands` and MCP.
    pub params: &'static str,
    /// Default shortcut (GPUI keystroke syntax), if any.
    pub shortcut: Option<&'static str>,
    /// Whether the command can change data or settings.
    pub mutating: bool,
}

macro_rules! cmd {
    ($id:literal, $title:literal, $cat:literal, $params:literal, $short:expr, $mut:expr) => {
        CommandSpec {
            id: $id,
            title: $title,
            category: $cat,
            params: $params,
            shortcut: $short,
            mutating: $mut,
        }
    };
}

pub fn builtin_commands() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "driver.list",
            "List drivers",
            "Connection",
            "{}",
            None,
            false
        ),
        cmd!(
            "connection.list",
            "List saved connections",
            "Connection",
            "{}",
            None,
            false
        ),
        cmd!(
            "connection.parseUrl",
            "Parse connection URL",
            "Connection",
            "{url}",
            None,
            false
        ),
        cmd!(
            "connection.save",
            "Save connection",
            "Connection",
            "ConnectionConfig",
            None,
            true
        ),
        cmd!(
            "connection.delete",
            "Delete connection",
            "Connection",
            "{id}",
            None,
            true
        ),
        cmd!(
            "connection.listFolders",
            "List connection groups",
            "Connection",
            "{}",
            None,
            false
        ),
        cmd!(
            "connection.createFolder",
            "New connection group",
            "Connection",
            "{name}",
            None,
            true
        ),
        cmd!(
            "connection.deleteFolder",
            "Delete connection group",
            "Connection",
            "{name} (its connections stay, outside any group)",
            None,
            true
        ),
        cmd!(
            "connection.moveToFolder",
            "Move connection to group",
            "Connection",
            "{id, folder?} (no folder: out of its group)",
            None,
            true
        ),
        cmd!(
            "connection.test",
            "Test connection",
            "Connection",
            "ConnectionConfig",
            None,
            false
        ),
        cmd!(
            "connection.open",
            "Connect",
            "Connection",
            "{id} | {url} | {config}",
            None,
            false
        ),
        cmd!(
            "connection.close",
            "Disconnect",
            "Connection",
            "{sessionId}",
            None,
            false
        ),
        cmd!(
            "session.list",
            "List open sessions",
            "Connection",
            "{}",
            None,
            false
        ),
        cmd!(
            "query.split",
            "Split into statements",
            "Query",
            "{sessionId, sql}",
            None,
            false
        ),
        cmd!(
            "query.params",
            "Find query parameters",
            "Query",
            "{sql}",
            None,
            false
        ),
        cmd!(
            "query.runAll",
            "Run all",
            "Query",
            "{sessionId, sql, params?}",
            Some("cmd-shift-enter"),
            true
        ),
        cmd!(
            "query.runCurrent",
            "Run current statement",
            "Query",
            "{sessionId, sql, cursor, params?}",
            Some("cmd-enter"),
            true
        ),
        cmd!(
            "query.runSelection",
            "Run selection",
            "Query",
            "{sessionId, sql, start, end, params?}",
            None,
            true
        ),
        cmd!(
            "schema.listObjects",
            "List schema objects",
            "Schema",
            "{sessionId}",
            None,
            false
        ),
        cmd!(
            "schema.describeTable",
            "Describe table",
            "Schema",
            "{sessionId, table}",
            None,
            false
        ),
        cmd!(
            "schema.refresh",
            "Refresh schema",
            "Schema",
            "{sessionId}",
            Some("cmd-r"),
            false
        ),
        cmd!(
            "structure.ddl",
            "Show DDL",
            "Structure",
            "{sessionId, table}",
            None,
            false
        ),
        cmd!(
            "table.fetchPage",
            "Fetch table page",
            "Table",
            "{sessionId, request: PageRequest}",
            None,
            false
        ),
        cmd!(
            "table.previewChanges",
            "Preview staged changes",
            "Table",
            "{sessionId, table, changes}",
            None,
            false
        ),
        cmd!(
            "table.applyChanges",
            "Apply staged changes",
            "Table",
            "{sessionId, table, changes}",
            Some("cmd-s"),
            true
        ),
        cmd!(
            "export.table",
            "Export table",
            "Export",
            "{sessionId, request, options, path?}",
            None,
            false
        ),
        cmd!(
            "result.export",
            "Export result",
            "Export",
            "{sessionId, result, options, path?}",
            None,
            false
        ),
        cmd!(
            "savedQuery.list",
            "List saved queries",
            "Saved queries",
            "{}",
            None,
            false
        ),
        cmd!(
            "savedQuery.save",
            "Save query",
            "Saved queries",
            "SavedQuery",
            None,
            true
        ),
        cmd!(
            "savedQuery.delete",
            "Delete saved query",
            "Saved queries",
            "{id}",
            None,
            true
        ),
        cmd!(
            "history.list",
            "Query history",
            "History",
            "{limit?}",
            None,
            false
        ),
        cmd!(
            "history.clear",
            "Clear history",
            "History",
            "{}",
            None,
            true
        ),
        // GUI-only commands are registered too so the palette and shortcuts list them.
        cmd!(
            "tab.new",
            "New query tab",
            "App",
            "{}",
            Some("cmd-t"),
            false
        ),
        cmd!("tab.close", "Close tab", "App", "{}", Some("cmd-w"), false),
        cmd!(
            "app.commandPalette",
            "Command palette",
            "App",
            "{}",
            Some("cmd-k"),
            false
        ),
        cmd!(
            "app.toggleTheme",
            "Toggle light/dark theme",
            "App",
            "{}",
            None,
            false
        ),
    ]
}

pub fn find(id: &str) -> Option<CommandSpec> {
    builtin_commands().into_iter().find(|c| c.id == id)
}

fn p<T: DeserializeOwned>(v: &Json) -> Result<T> {
    serde_json::from_value(v.clone()).map_err(|e| Error::InvalidParams(e.to_string()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sid {
    session_id: SessionId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TableArgs {
    session_id: SessionId,
    table: ObjectRef,
    #[serde(default)]
    changes: Box<[StagedChange]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenArgs {
    id: Option<String>,
    url: Option<String>,
    config: Option<ConnectionConfig>,
    #[serde(default)]
    read_only: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportArgs {
    session_id: SessionId,
    #[serde(default)]
    request: Option<PageRequest>,
    #[serde(default)]
    result: Option<ResultSet>,
    options: ExportOptions,
    #[serde(default)]
    path: Option<String>,
}

fn write_or_return(text: &str, path: Option<String>) -> Result<Json> {
    match path {
        Some(p) => {
            std::fs::write(&p, text)?;
            Ok(json!({ "path": p, "bytes": text.len() }))
        }
        None => Ok(json!({ "text": text })),
    }
}

/// Runs a command by id with JSON params and returns JSON.
pub fn dispatch(e: &Engine, id: &str, params: &Json) -> Result<Json> {
    let params = if params.is_null() { &json!({}) } else { params };
    let out = match id {
        "driver.list" => serde_json::to_value(e.drivers())?,
        "connection.list" => serde_json::to_value(e.connections())?,
        "connection.parseUrl" => {
            let url: String = p(&params["url"])?;
            serde_json::to_value(ConnectionConfig::from_url(&url)?)?
        }
        "connection.save" => serde_json::to_value(e.save_connection(p(params)?)?)?,
        "connection.delete" => {
            e.delete_connection(&p::<String>(&params["id"])?)?;
            json!({ "ok": true })
        }
        "connection.listFolders" => serde_json::to_value(e.folders())?,
        "connection.createFolder" => {
            serde_json::to_value(e.create_folder(&p::<String>(&params["name"])?)?)?
        }
        "connection.deleteFolder" => {
            serde_json::to_value(e.delete_folder(&p::<String>(&params["name"])?)?)?
        }
        "connection.moveToFolder" => {
            let id: String = p(&params["id"])?;
            let folder: Option<String> = p(&params["folder"]).unwrap_or(None);
            serde_json::to_value(e.move_connection(&id, folder.as_deref())?)?
        }
        "connection.test" => serde_json::to_value(e.test_connection(&p(params)?))?,
        "connection.open" => {
            let a: OpenArgs = p(params)?;
            let info = match (a.id, a.url, a.config) {
                (Some(id), _, _) => e.open_saved(&id)?,
                (_, Some(url), _) => {
                    let mut c = ConnectionConfig::from_url(&url)?;
                    c.read_only = a.read_only;
                    e.open(c)?
                }
                (_, _, Some(c)) => e.open(c)?,
                _ => return Err(Error::InvalidParams("need id, url or config".into())),
            };
            serde_json::to_value(info)?
        }
        "connection.close" => {
            e.close(p::<Sid>(params)?.session_id)?;
            json!({ "ok": true })
        }
        "session.list" => serde_json::to_value(e.sessions())?,
        "query.split" => {
            let sid = p::<Sid>(params)?.session_id;
            serde_json::to_value(e.split(sid, &p::<String>(&params["sql"])?)?)?
        }
        "query.params" => {
            let sql: String = p(&params["sql"])?;
            let keys: Vec<String> = crate::params::find(&sql).iter().map(|x| x.key()).collect();
            serde_json::to_value(keys)?
        }
        "query.runAll" | "query.runCurrent" | "query.runSelection" => {
            let mut req = params.clone();
            let scope = match id {
                "query.runCurrent" => json!({ "kind": "current", "cursor": params["cursor"] }),
                "query.runSelection" => {
                    json!({ "kind": "range", "start": params["start"], "end": params["end"] })
                }
                _ => json!({ "kind": "all" }),
            };
            req["scope"] = scope;
            let req: QueryRequest = p(&req)?;
            serde_json::to_value(e.run_query(&req)?)?
        }
        "schema.listObjects" | "schema.refresh" => {
            serde_json::to_value(e.list_objects(p::<Sid>(params)?.session_id)?)?
        }
        "schema.describeTable" => {
            let a: TableArgs = p(params)?;
            serde_json::to_value(e.describe_table(a.session_id, &a.table)?)?
        }
        "structure.ddl" => {
            let a: TableArgs = p(params)?;
            json!({ "ddl": e.ddl(a.session_id, &a.table)? })
        }
        "table.fetchPage" => {
            let sid = p::<Sid>(params)?.session_id;
            let req: PageRequest = p(&params["request"])?;
            serde_json::to_value(e.fetch_page(sid, &req)?)?
        }
        "table.previewChanges" => {
            let a: TableArgs = p(params)?;
            serde_json::to_value(e.preview_changes(a.session_id, &a.table, &a.changes)?)?
        }
        "table.applyChanges" => {
            let a: TableArgs = p(params)?;
            json!({ "rowsAffected": e.apply_changes(a.session_id, &a.table, &a.changes)? })
        }
        "export.table" => {
            let a: ExportArgs = p(params)?;
            let req = a
                .request
                .ok_or_else(|| Error::InvalidParams("request is required".into()))?;
            write_or_return(&e.export_table(a.session_id, &req, &a.options)?, a.path)?
        }
        "result.export" => {
            let a: ExportArgs = p(params)?;
            let rs = a
                .result
                .ok_or_else(|| Error::InvalidParams("result is required".into()))?;
            write_or_return(&e.export_result(a.session_id, &rs, &a.options)?, a.path)?
        }
        "savedQuery.list" => serde_json::to_value(e.saved_queries())?,
        "savedQuery.save" => serde_json::to_value(e.save_query(p::<SavedQuery>(params)?)?)?,
        "savedQuery.delete" => {
            e.delete_saved_query(&p::<String>(&params["id"])?)?;
            json!({ "ok": true })
        }
        "history.list" => {
            let limit = params["limit"].as_u64().unwrap_or(100) as usize;
            serde_json::to_value(e.history(limit))?
        }
        "history.clear" => {
            e.clear_history()?;
            json!({ "ok": true })
        }
        other if find(other).is_some() => {
            return Err(Error::Unsupported(format!(
                "`{other}` is handled by the GUI"
            )))
        }
        other => return Err(Error::UnknownCommand(other.into())),
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_namespaced() {
        let cmds = builtin_commands();
        let mut ids: Vec<_> = cmds.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), cmds.len());
        assert!(cmds.iter().all(|c| c.id.contains('.')));
    }

    #[test]
    fn unknown_command() {
        let e = Engine::new(crate::store::Store::in_memory());
        assert!(matches!(
            dispatch(&e, "nope.nope", &json!({})),
            Err(Error::UnknownCommand(_))
        ));
        let v = dispatch(&e, "query.params", &json!({"sql": "select :a, :b"})).unwrap();
        assert_eq!(v, json!(["a", "b"]));
    }
}
