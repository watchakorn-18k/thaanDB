//! Command registry. Every user-visible action has a stable [`CommandId`] and JSON
//! parameters; GUI, CLI, MCP and plugins all go through [`dispatch`].
//!
//! Never rename an id. Add a new one and keep the old as an alias if behaviour changes.

use crate::connection::ConnectionConfig;
use crate::engine::{Engine, QueryRequest, SessionId};
use crate::export::ExportOptions;
use crate::object_ops::ObjectOp;
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
            "{id, folder?, before?} (no folder: out of its group; before: place above that connection)",
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
            "{sessionId, database?}",
            None,
            false
        ),
        cmd!(
            "session.command",
            "Run a driver command",
            "Query",
            "{sessionId, command} (not kept in history)",
            None,
            false
        ),
        cmd!(
            "keys.scan",
            "Scan keys",
            "Keys",
            "{sessionId, pattern?, keyType?, cursor?, count?} → {keys: [{name, keyType, ttl?}], cursor, total?}",
            None,
            false
        ),
        cmd!(
            "broker.messages",
            "Read broker messages",
            "Broker",
            "{sessionId, since?, limit?} → [{id, atMs, topic, payload, size, qos, retain, sent}]",
            None,
            false
        ),
        cmd!(
            "broker.subscriptions",
            "List broker subscriptions",
            "Broker",
            "{sessionId}",
            None,
            false
        ),
        cmd!(
            "schema.listDatabases",
            "List databases",
            "Schema",
            "{sessionId}",
            None,
            false
        ),
        cmd!(
            "schema.setDatabase",
            "Switch database",
            "Schema",
            "{sessionId, database}",
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
            "structure.preview",
            "Preview a structure change",
            "Structure",
            "{sessionId, table: {schema?, name}, op} where op is one of {op: addColumn, name, dataType, nullable?, default?} | {op: dropColumn, name} | {op: alterColumn, name, renameTo?, from?, to?: {dataType, nullable?, default?}} | {op: createIndex, name, columns, unique?} | {op: dropIndex, name} | {op: addForeignKey, name?, columns, refTable: {name}, refColumns, onUpdate?, onDelete?} | {op: dropForeignKey, name}",
            None,
            false
        ),
        cmd!(
            "structure.apply",
            "Apply a structure change",
            "Structure",
            "{sessionId, table, op} (op as in structure.preview)",
            None,
            true
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
            "{sessionId, table, changes: [{kind: insert, values} | {kind: update, key, set} | {kind: delete, key}]} with values/key/set as [[column, {type: int|float|text|bool|bytes|null, value}]]",
            None,
            false
        ),
        cmd!(
            "table.applyChanges",
            "Apply staged changes",
            "Table",
            "{sessionId, table, changes} (changes as in table.previewChanges)",
            Some("cmd-s"),
            true
        ),
        cmd!(
            "table.previewOp",
            "Preview a table operation",
            "Table",
            "{sessionId, table, kind?, op: {op: rename|drop|truncate|duplicate, to?, withData?}}",
            None,
            false
        ),
        cmd!(
            "table.rename",
            "Rename table",
            "Table",
            "{sessionId, table, kind?, to}",
            None,
            true
        ),
        cmd!(
            "table.duplicate",
            "Duplicate table",
            "Table",
            "{sessionId, table, to, withData?}",
            None,
            true
        ),
        cmd!(
            "table.truncate",
            "Truncate table",
            "Table",
            "{sessionId, table}",
            None,
            true
        ),
        cmd!(
            "data.transfer",
            "Transfer data",
            "Table",
            "{source, target, tables: [ObjectRef], content?: structureAndData|structureOnly|dataOnly, existing?: append|truncate|replace, batchSize?}",
            None,
            true
        ),
        cmd!(
            "collection.create",
            "Create collection",
            "Table",
            "{sessionId, name}",
            None,
            true
        ),
        cmd!(
            "table.drop",
            "Drop table",
            "Table",
            "{sessionId, table, kind?}",
            None,
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
        cmd!(
            "settings.get",
            "Show settings",
            "Settings",
            "{}",
            None,
            false
        ),
        cmd!(
            "settings.set",
            "Change settings",
            "Settings",
            "{theme?: system|light|dark, fontSize?, pageSize?, historyLimit?}",
            None,
            false
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
            "app.settings",
            "Settings",
            "App",
            "{}",
            Some("cmd-,"),
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
struct OpArgs {
    session_id: SessionId,
    table: ObjectRef,
    #[serde(default = "table_kind")]
    kind: crate::schema::ObjectKind,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    with_data: Option<bool>,
}

fn table_kind() -> crate::schema::ObjectKind {
    crate::schema::ObjectKind::Table
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
            let before: Option<String> = p(&params["before"]).unwrap_or(None);
            let c = match before {
                Some(b) => e.place_connection(&id, folder.as_deref(), Some(&b))?,
                None => e.move_connection(&id, folder.as_deref())?,
            };
            serde_json::to_value(c)?
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
            let sid = p::<Sid>(params)?.session_id;
            match params["database"].as_str() {
                Some(db) => serde_json::to_value(e.list_objects_in(sid, db)?)?,
                None => serde_json::to_value(e.list_objects(sid)?)?,
            }
        }
        "session.command" => {
            let sid = p::<Sid>(params)?.session_id;
            let text: String = p(&params["command"])?;
            serde_json::to_value(e.driver_command(sid, &text)?)?
        }
        "keys.scan" => {
            let sid = p::<Sid>(params)?.session_id;
            let scan: crate::browse::KeyScan = p(params)?;
            serde_json::to_value(e.scan_keys(sid, &scan)?)?
        }
        "broker.messages" => {
            let sid = p::<Sid>(params)?.session_id;
            let since: u64 = params.get("since").map(p).transpose()?.unwrap_or(0);
            let limit: usize = params.get("limit").map(p).transpose()?.unwrap_or(1000);
            serde_json::to_value(e.messages_since(sid, since, limit)?)?
        }
        "broker.subscriptions" => {
            serde_json::to_value(e.subscriptions(p::<Sid>(params)?.session_id)?)?
        }
        "schema.listDatabases" => {
            serde_json::to_value(e.list_databases(p::<Sid>(params)?.session_id)?)?
        }
        "schema.setDatabase" => {
            let sid = p::<Sid>(params)?.session_id;
            let db: String = p(&params["database"])?;
            serde_json::to_value(e.switch_database(sid, &db)?)?
        }
        "schema.describeTable" => {
            let a: TableArgs = p(params)?;
            serde_json::to_value(e.describe_table(a.session_id, &a.table)?)?
        }
        "structure.ddl" => {
            let a: TableArgs = p(params)?;
            json!({ "ddl": e.ddl(a.session_id, &a.table)? })
        }
        "structure.preview" | "structure.apply" => {
            let a: TableArgs = p(params)?;
            let op: crate::structure_ops::StructureOp = p(&params["op"])?;
            if id == "structure.preview" {
                json!({ "statements": e.structure_op_statements(a.session_id, &a.table, &op)? })
            } else {
                e.run_structure_op(a.session_id, &a.table, &op)?;
                json!({ "ok": true })
            }
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
        "table.previewOp" => {
            let a: OpArgs = p(params)?;
            let op: ObjectOp = p(&params["op"])?;
            json!({ "statements": e.object_op_statements(a.session_id, a.kind, &a.table, &op)? })
        }
        "data.transfer" => {
            let req: crate::transfer::TransferRequest = p(params)?;
            let never = std::sync::atomic::AtomicBool::new(false);
            serde_json::to_value(e.transfer(&req, &|_| {}, &never)?)?
        }
        "collection.create" => {
            let sid = p::<Sid>(params)?.session_id;
            let name: String = p(&params["name"])?;
            let object = ObjectRef { schema: None, name };
            e.run_object_op(
                sid,
                crate::schema::ObjectKind::Table,
                &object,
                &ObjectOp::Create,
            )?;
            json!({ "ok": true })
        }
        "table.rename" | "table.duplicate" | "table.truncate" | "table.drop" => {
            let a: OpArgs = p(params)?;
            let to = || {
                a.to.clone()
                    .ok_or_else(|| Error::InvalidParams("`to` is required".into()))
            };
            let op = match id {
                "table.rename" => ObjectOp::Rename { to: to()? },
                "table.duplicate" => ObjectOp::Duplicate {
                    to: to()?,
                    with_data: a.with_data.unwrap_or(true),
                },
                "table.truncate" => ObjectOp::Truncate,
                _ => ObjectOp::Drop,
            };
            e.run_object_op(a.session_id, a.kind, &a.table, &op)?;
            json!({ "ok": true })
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
        "settings.get" => serde_json::to_value(e.settings())?,
        "settings.set" => {
            // Fields left out keep their current value.
            let mut merged = serde_json::to_value(e.settings())?;
            if let (Some(m), Some(new)) = (merged.as_object_mut(), params.as_object()) {
                for (k, v) in new {
                    m.insert(k.clone(), v.clone());
                }
            }
            serde_json::to_value(e.set_settings(p::<crate::store::Settings>(&merged)?)?)?
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
