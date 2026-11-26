//! MCP (Model Context Protocol) server logic: JSON-RPC 2.0 messages in, messages out.
//!
//! Written from the public MCP specification (modelcontextprotocol.io). This module does no
//! I/O; a transport (the `thaan mcp` stdio loop in `thaan-cli`) feeds it one message at a
//! time with [`McpServer::handle`] and writes back whatever it returns.
//!
//! The tools are deliberately few and forgiving:
//! - every tool that needs a database takes `connection`: a saved connection's id or name,
//!   or a URL (`sqlite:./app.db`, `postgres://…`). The session is opened on first use and
//!   reused after that, so there is no connect step and no session id to carry around;
//! - `run_command` reaches every command in the registry, so the agent can do anything the
//!   GUI can do.
//!
//! There are no permission tiers. Read-only connections stay read-only: the engine refuses
//! writes on them whoever asks.

use crate::commands::{builtin_commands, dispatch};
use crate::connection::ConnectionConfig;
use crate::engine::{Engine, QueryRequest, RunScope, SessionId};
use crate::schema::ObjectRef;
use crate::table::{PageRequest, Sort};
use crate::{Error, Result, ResultSet, Value};
use serde_json::{json, Map, Value as Json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Protocol revisions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Rows a `query` returns unless the caller asks for more.
pub const DEFAULT_MAX_ROWS: usize = 200;
/// Hard ceiling for `maxRows`, to keep replies (and the agent's context) small.
pub const MAX_ROWS_LIMIT: usize = 5_000;
/// Text cells longer than this are shortened in replies.
pub const MAX_CELL_CHARS: usize = 2_000;

const INSTRUCTIONS: &str = "ThaanDB: query and manage SQL (SQLite, PostgreSQL, MySQL/MariaDB), \
MongoDB, Redis and MQTT databases. Start with list_connections. Every tool takes `connection`: \
a saved connection's name or id, or a URL such as sqlite:./app.db or \
postgres://user:pw@host/db; it opens on first use and stays open. `query` runs SQL \
(MongoDB: shell syntax such as db.users.find({}); Redis: commands). Use run_command with \
list_commands for anything else (structure changes, staged edits, transfers, export, saved \
queries, history, settings).";

pub struct McpServer {
    engine: Arc<Engine>,
    /// `connection` argument as given → open session.
    sessions: Mutex<HashMap<String, SessionId>>,
}

impl McpServer {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Handles one JSON-RPC message. Returns the response, or `None` for notifications and
    /// responses (which need no answer).
    pub fn handle(&self, msg: &Json) -> Option<Json> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Json::as_str);
        let (Some(id), Some(method)) = (id, method) else {
            if method.is_none() && msg.get("id").is_none() {
                return Some(rpc_error(Json::Null, -32600, "invalid request"));
            }
            return None; // notification, or a response to us
        };
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        Some(match self.request(method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => rpc_error(id, code, &message),
        })
    }

    /// Parses one line of the stdio transport and handles it.
    pub fn handle_line(&self, line: &str) -> Option<Json> {
        match serde_json::from_str::<Json>(line) {
            Ok(Json::Array(batch)) => {
                // Older revisions allow batches; answer each request in one array.
                let out: Vec<Json> = batch.iter().filter_map(|m| self.handle(m)).collect();
                (!out.is_empty()).then_some(Json::Array(out))
            }
            Ok(msg) => self.handle(&msg),
            Err(e) => Some(rpc_error(Json::Null, -32700, &format!("parse error: {e}"))),
        }
    }

    fn request(&self, method: &str, params: &Json) -> std::result::Result<Json, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "thaandb",
                        "title": "ThaanDB",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params["name"]
                    .as_str()
                    .ok_or((-32602, "missing tool name".to_string()))?;
                if !tools().iter().any(|t| t["name"] == name) {
                    return Err((-32602, format!("unknown tool `{name}`")));
                }
                let args = match &params["arguments"] {
                    Json::Null => json!({}),
                    a => a.clone(),
                };
                // Tool failures are results with isError, so the agent can read and fix them.
                // Every reply and error passes the redaction: no connection secret ever
                // reaches the agent (see `redact`).
                let known = self.known_secrets();
                Ok(match self.call(name, &args) {
                    Ok(v) => tool_result(&redact(&v, &known), false),
                    Err(e) => tool_result(
                        &json!({ "error": redact_text(&e.to_string(), &known) }),
                        true,
                    ),
                })
            }
            "resources/list" => Ok(json!({ "resources": [] })),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err((-32601, format!("method not found: {other}"))),
        }
    }

    /// Runs one tool with its arguments.
    pub fn call(&self, name: &str, a: &Json) -> Result<Json> {
        let e = &*self.engine;
        match name {
            "list_connections" => {
                let saved: Vec<Json> = e
                    .connections()
                    .into_iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "name": c.name,
                            "driver": c.driver,
                            "target": target(&c),
                            "folder": c.folder,
                            "readOnly": c.read_only,
                        })
                    })
                    .collect();
                Ok(json!({ "connections": saved, "open": e.sessions() }))
            }
            "add_connection" => {
                let mut c = match (str_arg(a, "url"), a.get("config")) {
                    (Some(url), _) => ConnectionConfig::from_url(url)?,
                    (None, Some(cfg)) => serde_json::from_value(cfg.clone())
                        .map_err(|e| Error::InvalidParams(e.to_string()))?,
                    _ => return Err(Error::InvalidParams("give `url` or `config`".into())),
                };
                if let Some(n) = str_arg(a, "name") {
                    c.name = n.to_string();
                }
                if let Some(ro) = a["readOnly"].as_bool() {
                    c.read_only = ro;
                }
                if let Some(f) = str_arg(a, "folder") {
                    c.folder = Some(f.to_string());
                }
                let test = a["test"].as_bool().unwrap_or(true);
                if test {
                    let t = e.test_connection(&c);
                    if !t.ok {
                        return Err(Error::Database(format!(
                            "connection test failed, not saved: {}",
                            t.error.unwrap_or_default()
                        )));
                    }
                }
                let saved = e.save_connection(c)?;
                Ok(json!({ "id": saved.id, "name": saved.name, "driver": saved.driver }))
            }
            "query" => {
                let sid = self.session(a)?;
                let sql = str_arg(a, "sql")
                    .ok_or_else(|| Error::InvalidParams("`sql` is required".into()))?;
                let max = max_rows(a);
                let results = e.run_query(&QueryRequest {
                    session_id: sid,
                    sql: sql.to_string(),
                    scope: RunScope::All,
                    params: query_params(&a["params"]),
                })?;
                let out: Vec<Json> = results
                    .into_iter()
                    .map(|r| match (r.result, r.error) {
                        (_, Some(err)) => json!({ "sql": r.sql, "error": err }),
                        (Some(rs), None) => result_json(&rs, max),
                        (None, None) => json!({ "sql": r.sql }),
                    })
                    .collect();
                Ok(json!({ "results": out }))
            }
            "list_databases" => {
                let sid = self.session(a)?;
                Ok(serde_json::to_value(e.list_databases(sid)?)?)
            }
            "list_tables" => {
                let sid = self.session(a)?;
                let objs = match str_arg(a, "database") {
                    Some(db) => e.list_objects_in(sid, db)?,
                    None => e.list_objects(sid)?,
                };
                let objs: Vec<Json> = objs
                    .into_iter()
                    .filter(|o| match str_arg(a, "schema") {
                        Some(s) => o.object.schema.as_deref() == Some(s),
                        None => true,
                    })
                    .map(|o| {
                        let mut v = json!({
                            "name": o.object.name,
                            "kind": o.kind,
                        });
                        if let Some(s) = o.object.schema {
                            v["schema"] = s.into();
                        }
                        if let Some(p) = o.parent {
                            v["parent"] = p.into();
                        }
                        v
                    })
                    .collect();
                Ok(json!({ "objects": objs }))
            }
            "describe_table" => {
                let sid = self.session(a)?;
                let t = table_ref(a)?;
                let mut v = serde_json::to_value(e.describe_table(sid, &t)?)?;
                if a["ddl"].as_bool().unwrap_or(true) {
                    // Some drivers have no DDL (documents, keys); the columns are enough then.
                    if let Ok(ddl) = e.ddl(sid, &t) {
                        v["ddl"] = ddl.into();
                    }
                }
                Ok(v)
            }
            "fetch_rows" => {
                let sid = self.session(a)?;
                let page_size = a["pageSize"]
                    .as_u64()
                    .unwrap_or(50)
                    .clamp(1, MAX_ROWS_LIMIT as u64);
                let sort = match &a["sort"] {
                    Json::Array(items) => items
                        .iter()
                        .filter_map(|s| match s {
                            Json::String(c) => Some(Sort {
                                column: c.trim_start_matches('-').to_string(),
                                descending: c.starts_with('-'),
                            }),
                            Json::Object(_) => serde_json::from_value(s.clone()).ok(),
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                };
                let req = PageRequest {
                    table: table_ref(a)?,
                    filters: vec![],
                    raw_where: str_arg(a, "where").map(str::to_string),
                    projection: str_arg(a, "projection").map(str::to_string),
                    raw_sort: str_arg(a, "rawSort").map(str::to_string),
                    sort,
                    page: a["page"].as_u64().unwrap_or(0),
                    page_size,
                };
                let page = e.fetch_page(sid, &req)?;
                let mut v = result_json(&page.result, page_size as usize);
                v["totalRows"] = page.total_rows.into();
                v["page"] = page.page.into();
                v["pageCount"] = page.page_count().into();
                Ok(v)
            }
            "disconnect" => {
                let key = conn_key(a)?;
                let sid = self.lock_sessions().remove(&key);
                let sid = match sid {
                    Some(s) => s,
                    None => key
                        .parse()
                        .map_err(|_| Error::UnknownConnection(key.clone()))?,
                };
                e.close(sid)?;
                Ok(json!({ "ok": true }))
            }
            "list_commands" => {
                let cmds: Vec<Json> = builtin_commands()
                    .into_iter()
                    .filter(|c| c.category != "App")
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "title": c.title,
                            "params": c.params,
                            "mutating": c.mutating,
                        })
                    })
                    .collect();
                Ok(json!({ "commands": cmds }))
            }
            "run_command" => {
                let id = str_arg(a, "command")
                    .ok_or_else(|| Error::InvalidParams("`command` is required".into()))?;
                let mut params = match &a["params"] {
                    Json::Null => json!({}),
                    p => p.clone(),
                };
                if a.get("connection").is_some() && params.get("sessionId").is_none() {
                    params["sessionId"] = self.session(a)?.into();
                }
                dispatch(e, id, &params)
            }
            other => Err(Error::UnknownCommand(other.into())),
        }
    }

    /// Secret values this process holds in memory: saved connections' passwords (the
    /// store keeps them for the run) and those filled into open sessions from the
    /// keychain. Longest first, so a longer secret is masked before a part of it.
    fn known_secrets(&self) -> Vec<String> {
        let e = &*self.engine;
        let mut out = Vec::new();
        let mut add = |c: &ConnectionConfig| {
            for (k, v) in &c.params {
                if is_secret_key(k) && !v.is_empty() {
                    out.push(v.clone());
                }
            }
        };
        for c in e.connections() {
            add(&c);
        }
        for s in e.sessions() {
            if let Ok(open) = e.session(s.session_id) {
                add(&open.connection);
            }
        }
        out.sort_by_key(|v| std::cmp::Reverse(v.len()));
        out.dedup();
        out
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, SessionId>> {
        self.sessions.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The open session for the call's `connection` (or `sessionId`), opening it if needed.
    fn session(&self, a: &Json) -> Result<SessionId> {
        if let Some(sid) = a["sessionId"].as_u64() {
            return Ok(sid);
        }
        let key = conn_key(a)?;
        let alive = |sid: SessionId| self.engine.session(sid).is_ok();
        if let Some(&sid) = self.lock_sessions().get(&key) {
            if alive(sid) {
                return Ok(sid);
            }
        }
        // A bare number is a session id from list_connections' `open`.
        if let Ok(sid) = key.parse::<SessionId>() {
            if alive(sid) {
                return Ok(sid);
            }
        }
        let saved = self
            .engine
            .connections()
            .into_iter()
            .find(|c| c.id == key || c.name.eq_ignore_ascii_case(&key));
        let config = match saved {
            Some(c) => c,
            None if key.contains(':') => ConnectionConfig::from_url(&key)?,
            None => return Err(Error::UnknownConnection(key)),
        };
        // Opening can take a while (network); the map is not locked meanwhile.
        let sid = match self.engine.open(config.clone()) {
            Ok(info) => info.session_id,
            Err(e) => {
                // The engine filled secrets from the keychain into its own copy; a
                // driver error may quote them, and they are not in memory anywhere the
                // reply redaction looks. Mask them here (a keychain read, error path only).
                let mut secrets: Vec<String> = crate::store::SECRET_KEYS
                    .iter()
                    .filter_map(|k| self.engine.secret(&config.id, k))
                    .chain(
                        config
                            .params
                            .iter()
                            .filter(|(k, _)| is_secret_key(k))
                            .map(|(_, v)| v.clone()),
                    )
                    .filter(|v| !v.is_empty())
                    .collect();
                secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
                return Err(Error::Database(redact_text(&e.to_string(), &secrets)));
            }
        };
        self.lock_sessions().insert(key, sid);
        Ok(sid)
    }
}

// ---- secrets ----------------------------------------------------------------------------

/// What a masked secret reads as.
pub const MASK: &str = "••••••";

/// Parameter names that hold secrets: the store's own list and anything named like one.
fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    crate::store::SECRET_KEYS
        .iter()
        .any(|s| s.eq_ignore_ascii_case(&k))
        || ["password", "passwd", "passphrase", "secret", "token", "apikey", "api_key"]
            .iter()
            .any(|s| k.contains(s))
        || k == "pwd"
}

/// Masks connection secrets anywhere in a reply:
/// - the value of any field named like a secret (`password`, `sshPassphrase`, …);
/// - in every string, the known secret values (`known_secrets`), the password part of
///   URLs (`postgres://u:•••@h`) and `password=…` / `PASSWORD '…'` / `IDENTIFIED BY '…'`.
///
/// `rows` (query and page results) are the database's own data and are left as they
/// are: masking every occurrence of a short password there would damage real values.
pub fn redact(v: &Json, known: &[String]) -> Json {
    match v {
        Json::String(s) => Json::String(redact_text(s, known)),
        Json::Array(items) => Json::Array(items.iter().map(|x| redact(x, known)).collect()),
        Json::Object(m) => Json::Object(
            m.iter()
                .map(|(k, x)| {
                    let x = if is_secret_key(k) && !x.is_null() && x != "" {
                        Json::String(MASK.into())
                    } else if k == "rows" || k == "documents" {
                        x.clone()
                    } else {
                        redact(x, known)
                    };
                    (k.clone(), x)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// [`redact`] for one piece of text.
pub fn redact_text(s: &str, known: &[String]) -> String {
    let mut out = s.to_string();
    for secret in known {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), MASK);
        }
    }
    mask_url_passwords(&mask_assignments(&out))
}

/// `scheme://user:password@host` → `scheme://user:••••••@host`.
fn mask_url_passwords(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        // The authority ends at the first `/`, whitespace or quote.
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '"' || c == '\'' || c == '?')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                let userinfo = &authority[..at];
                match userinfo.find(':') {
                    Some(colon) => {
                        out.push_str(&userinfo[..=colon]);
                        out.push_str(MASK);
                    }
                    None => out.push_str(userinfo),
                }
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Masks the value after `password=`, `pwd:`, `"password": "…"`, and the quoted value
/// after SQL's `PASSWORD '…'` and `IDENTIFIED BY '…'`.
fn mask_assignments(s: &str) -> String {
    const WORDS: [&str; 5] = ["identified by", "passphrase", "password", "passwd", "pwd"];
    let lower = s.to_ascii_lowercase();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'scan: while i < s.len() {
        for w in WORDS {
            if !lower[i..].starts_with(w) {
                continue;
            }
            // A whole word: not the middle of `mypasswords_table`.
            let before_ok = i == 0 || !(bytes[i - 1] as char).is_ascii_alphanumeric();
            if !before_ok {
                break;
            }
            let mut j = i + w.len();
            // `"password": "…"` in JSON-looking text: skip the key's closing quote.
            if j < s.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                j += 1;
            }
            let ws = |j: &mut usize| {
                while *j < s.len() && (bytes[*j] == b' ' || bytes[*j] == b'\t') {
                    *j += 1;
                }
            };
            ws(&mut j);
            let assign = j < s.len() && (bytes[j] == b'=' || bytes[j] == b':');
            if assign {
                j += 1;
                ws(&mut j);
            }
            let quoted = j < s.len() && (bytes[j] == b'\'' || bytes[j] == b'"');
            // Prose like "password missing" has neither `=` / `:` nor a quote.
            if !assign && !quoted {
                break;
            }
            let (value_start, value_end) = if quoted {
                let q = bytes[j];
                let end = s[j + 1..]
                    .find(q as char)
                    .map_or(s.len(), |e| j + 1 + e);
                (j + 1, end)
            } else {
                let end = s[j..]
                    .find(|c: char| c.is_whitespace() || ";&,)\"'".contains(c))
                    .map_or(s.len(), |e| j + e);
                (j, end)
            };
            if value_end > value_start {
                out.push_str(&s[i..value_start]);
                out.push_str(MASK);
                i = value_end;
                continue 'scan;
            }
            break;
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn rpc_error(id: Json, code: i64, message: &str) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_result(v: &Json, is_error: bool) -> Json {
    let text = serde_json::to_string(v).unwrap_or_default();
    let mut r = json!({ "content": [{ "type": "text", "text": text }], "isError": is_error });
    if v.is_object() {
        r["structuredContent"] = v.clone();
    }
    r
}

fn str_arg<'a>(a: &'a Json, k: &str) -> Option<&'a str> {
    a.get(k)
        .and_then(Json::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn conn_key(a: &Json) -> Result<String> {
    match &a["connection"] {
        Json::String(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Json::Number(n) => Ok(n.to_string()),
        _ => Err(Error::InvalidParams(
            "`connection` is required: a saved connection's name or id, or a URL".into(),
        )),
    }
}

fn table_ref(a: &Json) -> Result<ObjectRef> {
    let name =
        str_arg(a, "table").ok_or_else(|| Error::InvalidParams("`table` is required".into()))?;
    let schema = str_arg(a, "schema").map(str::to_string);
    // `schema.table` works too, when no schema is given separately.
    Ok(match (schema, name.split_once('.')) {
        (None, Some((s, n))) if !n.is_empty() && !name.contains('"') => ObjectRef {
            schema: Some(s.to_string()),
            name: n.to_string(),
        },
        (schema, _) => ObjectRef {
            schema,
            name: name.to_string(),
        },
    })
}

fn max_rows(a: &Json) -> usize {
    a["maxRows"]
        .as_u64()
        .map_or(DEFAULT_MAX_ROWS, |n| n as usize)
        .clamp(1, MAX_ROWS_LIMIT)
}

/// `params` as `{name: value}` or a list for `?` / `$1` placeholders.
fn query_params(p: &Json) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    match p {
        Json::Object(m) => {
            for (k, v) in m {
                out.insert(k.trim_start_matches(':').to_string(), Value::from_json(v));
            }
        }
        Json::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                out.insert(format!("?{}", i + 1), Value::from_json(v));
                out.insert(format!("${}", i + 1), Value::from_json(v));
            }
        }
        _ => {}
    }
    out
}

fn short(v: &Value) -> Json {
    match v {
        Value::Text(s) if s.chars().count() > MAX_CELL_CHARS => {
            let cut: String = s.chars().take(MAX_CELL_CHARS).collect();
            format!("{cut}… ({} chars)", s.chars().count()).into()
        }
        other => other.to_json(),
    }
}

/// A result as compact JSON: column names, rows as objects, at most `max` rows.
fn result_json(rs: &ResultSet, max: usize) -> Json {
    let mut v = json!({ "sql": rs.sql, "elapsedMs": rs.elapsed_ms });
    if rs.columns.is_empty() {
        v["rowsAffected"] = rs.rows_affected.unwrap_or(0).into();
        return v;
    }
    let rows: Vec<Json> = rs
        .rows
        .iter()
        .take(max)
        .map(|row| {
            let mut obj = Map::new();
            for (c, cell) in rs.columns.iter().zip(row) {
                obj.insert(c.name.clone(), short(cell));
            }
            Json::Object(obj)
        })
        .collect();
    v["columns"] = rs
        .columns
        .iter()
        .map(|c| match &c.type_name {
            Some(t) => json!({ "name": c.name, "type": t }),
            None => json!({ "name": c.name }),
        })
        .collect();
    v["rowCount"] = rs.rows.len().into();
    if rs.rows.len() > max {
        v["truncated"] = true.into();
        v["note"] = format!(
            "showing the first {max} of {} rows; raise maxRows or add LIMIT/OFFSET",
            rs.rows.len()
        )
        .into();
    }
    v["rows"] = rows.into();
    if let Some(n) = rs.rows_affected {
        v["rowsAffected"] = n.into();
    }
    v
}

/// Where a saved connection points, without secrets.
fn target(c: &ConnectionConfig) -> String {
    let p = |k: &str| c.param(k).unwrap_or_default();
    match c.driver.as_str() {
        "sqlite" => p("path").to_string(),
        _ => {
            let host = p("host");
            let port = c.param("port").map(|x| format!(":{x}")).unwrap_or_default();
            let db = c
                .param("database")
                .map(|d| format!("/{d}"))
                .unwrap_or_default();
            format!("{host}{port}{db}")
        }
    }
}

fn conn_prop() -> Json {
    json!({
        "type": "string",
        "description": "Saved connection name or id, or a URL (sqlite:./app.db, sqlite::memory:, postgres://user:pw@host/db, mysql://…, mongodb://…, redis://…). Opens on first use and stays open."
    })
}

fn table_props() -> (Json, Json) {
    (
        json!({ "type": "string", "description": "Table, view or collection name (`schema.table` works too)" }),
        json!({ "type": "string", "description": "Schema, when not in `table`" }),
    )
}

/// The tool catalog sent by `tools/list`.
pub fn tools() -> Vec<Json> {
    let (table, schema) = table_props();
    let read = json!({ "readOnlyHint": true, "openWorldHint": false });
    vec![
        json!({
            "name": "list_connections",
            "title": "List connections",
            "description": "Saved connections (name, id, driver, host/db) and the sessions open now. Start here.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read,
        }),
        json!({
            "name": "add_connection",
            "title": "Add connection",
            "description": "Save a connection so it can be used by name later. Tested first unless test=false.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Connection URL, e.g. postgres://user:pw@localhost:5432/shop" },
                    "name": { "type": "string", "description": "Display name (defaults from the URL)" },
                    "readOnly": { "type": "boolean", "description": "Refuse writes on this connection" },
                    "folder": { "type": "string", "description": "Connection group" },
                    "test": { "type": "boolean", "description": "Connect once before saving (default true)" },
                    "config": { "type": "object", "description": "Full ConnectionConfig instead of url: {name, driver, params:{host,port,user,password,database,…}}" }
                }
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false },
        }),
        json!({
            "name": "query",
            "title": "Run query",
            "description": "Run one or more statements and return their results (rows as objects, capped at maxRows). SQL for SQL databases; MongoDB shell syntax (db.users.find({age:{$gt:30}}).limit(10)); Redis commands (GET key). Stops at the first failing statement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection": conn_prop(),
                    "sql": { "type": "string", "description": "Statements to run, separated by `;`" },
                    "params": { "description": "Values for :name placeholders as an object, or a list for ? / $1", "type": ["object", "array"] },
                    "maxRows": { "type": "integer", "description": "Rows returned per statement (default 200, max 5000)" }
                },
                "required": ["connection", "sql"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true },
        }),
        json!({
            "name": "list_databases",
            "title": "List databases",
            "description": "Databases on the server and the current one.",
            "inputSchema": {
                "type": "object",
                "properties": { "connection": conn_prop() },
                "required": ["connection"]
            },
            "annotations": read,
        }),
        json!({
            "name": "list_tables",
            "title": "List tables",
            "description": "Tables, views, indexes, triggers (or collections / keys) of the current database, or of `database`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection": conn_prop(),
                    "database": { "type": "string", "description": "Another database on the same server" },
                    "schema": { "type": "string", "description": "Only objects in this schema" }
                },
                "required": ["connection"]
            },
            "annotations": read,
        }),
        json!({
            "name": "describe_table",
            "title": "Describe table",
            "description": "Columns (type, nullable, default, primary key), foreign keys, indexes, triggers and the CREATE statement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection": conn_prop(),
                    "table": table,
                    "schema": schema,
                    "ddl": { "type": "boolean", "description": "Include the CREATE statement (default true)" }
                },
                "required": ["connection", "table"]
            },
            "annotations": read,
        }),
        json!({
            "name": "fetch_rows",
            "title": "Fetch rows",
            "description": "One page of a table or collection, with total row count. Filter with a WHERE expression (SQL) or a filter document (MongoDB).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection": conn_prop(),
                    "table": table,
                    "schema": schema,
                    "where": { "type": "string", "description": "SQL: WHERE text without the keyword, e.g. age > 30. MongoDB: filter document, e.g. { age: { $gt: 30 } }" },
                    "sort": { "type": "array", "items": { "type": "string" }, "description": "Columns to sort by; prefix with - for descending, e.g. [\"-created_at\", \"id\"]" },
                    "projection": { "type": "string", "description": "MongoDB projection document" },
                    "page": { "type": "integer", "description": "0-based page (default 0)" },
                    "pageSize": { "type": "integer", "description": "Rows per page (default 50, max 5000)" }
                },
                "required": ["connection", "table"]
            },
            "annotations": read,
        }),
        json!({
            "name": "disconnect",
            "title": "Disconnect",
            "description": "Close the open session of a connection.",
            "inputSchema": {
                "type": "object",
                "properties": { "connection": conn_prop() },
                "required": ["connection"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false },
        }),
        json!({
            "name": "list_commands",
            "title": "List commands",
            "description": "Every engine command id with its parameters, for run_command.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read,
        }),
        json!({
            "name": "run_command",
            "title": "Run command",
            "description": "Run any engine command by id (see list_commands): structure changes (structure.preview / structure.apply), staged row edits (table.previewChanges / table.applyChanges), rename / duplicate / truncate / drop, data.transfer, export, saved queries, history, settings. Give `connection` and sessionId is filled in.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command id, e.g. structure.apply" },
                    "params": { "type": "object", "description": "The command's JSON parameters" },
                    "connection": conn_prop()
                },
                "required": ["command"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true },
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn server() -> McpServer {
        McpServer::new(Arc::new(Engine::new(Store::in_memory())))
    }

    fn call(s: &McpServer, id: u64, method: &str, params: Json) -> Json {
        s.handle(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .expect("requests get a response")
    }

    #[test]
    fn initialize_negotiates_version() {
        let s = server();
        let r = call(
            &s,
            1,
            "initialize",
            json!({ "protocolVersion": "2025-03-26" }),
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "thaandb");
        let r = call(
            &s,
            2,
            "initialize",
            json!({ "protocolVersion": "1999-01-01" }),
        );
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply() {
        let s = server();
        assert!(s
            .handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .is_none());
        let r = s.handle_line("{not json").unwrap();
        assert_eq!(r["error"]["code"], -32700);
        let r = call(&s, 3, "nope/nope", json!({}));
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn tool_names_are_unique_and_have_schemas() {
        let t = tools();
        let mut names: Vec<_> = t.iter().map(|x| x["name"].as_str().unwrap()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), t.len());
        assert!(t.iter().all(|x| x["inputSchema"]["type"] == "object"));
    }

    #[test]
    fn tool_errors_are_results() {
        let s = server();
        let r = call(
            &s,
            4,
            "tools/call",
            json!({ "name": "query", "arguments": {} }),
        );
        assert_eq!(r["result"]["isError"], true);
        let r = call(&s, 5, "tools/call", json!({ "name": "no_such_tool" }));
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn helpers() {
        let t = table_ref(&json!({ "table": "public.users" })).unwrap();
        assert_eq!(t.schema.as_deref(), Some("public"));
        assert_eq!(t.name, "users");
        let p = query_params(&json!([1, "x"]));
        assert_eq!(p["?2"], Value::Text("x".into()));
        let p = query_params(&json!({ ":id": 7 }));
        assert_eq!(p["id"], Value::Int(7));
        let rs = ResultSet {
            columns: vec![crate::Column::new("n")],
            rows: (0..5).map(|i| vec![Value::Int(i)]).collect(),
            ..Default::default()
        };
        let v = result_json(&rs, 2);
        assert_eq!(v["rows"].as_array().unwrap().len(), 2);
        assert_eq!(v["truncated"], true);
        assert_eq!(v["rowCount"], 5);
    }

    #[test]
    fn secrets_never_leave() {
        let known = vec!["hunter22".to_string()];
        let v = redact(
            &json!({
                "params": { "password": "hunter22", "sshPassphrase": "x", "host": "db" },
                "error": "cannot reach postgres://app:hunter22@db:5432/shop",
                "sql": "ALTER USER app PASSWORD 'n3w-pw'; CREATE USER b IDENTIFIED BY \"q\"",
                "url": "mysql://root:abc@localhost/app and host=x password=xyz;port=1",
                "rows": [["hunter22"]],
                "note": "password missing"
            }),
            &known,
        );
        let text = v.to_string();
        for leak in ["hunter22\"}", ":hunter22@", "n3w-pw", "abc@", "xyz", "\"x\""] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
        assert_eq!(v["params"]["password"], MASK);
        assert_eq!(v["params"]["host"], "db");
        assert_eq!(v["note"], "password missing");
        assert!(v["error"].as_str().unwrap().contains("postgres://app:••••••@db:5432/shop"));
        // Query rows are the database's data and stay as they are.
        assert_eq!(v["rows"][0][0], "hunter22");
    }

    /// Never connects; enough to save connections.
    struct NullDriver;
    impl crate::driver::Driver for NullDriver {
        fn info(&self) -> crate::driver::DriverInfo {
            crate::driver::DriverInfo {
                id: "null".into(),
                name: "Null".into(),
                params: Vec::new(),
            }
        }
        fn dialect(&self) -> crate::dialect::Dialect {
            crate::dialect::Dialect::for_driver("sqlite")
        }
        fn connect(
            &self,
            c: &ConnectionConfig,
        ) -> Result<Box<dyn crate::driver::Session>> {
            // A driver error that quotes the password, the worst case.
            Err(Error::Database(format!(
                "login failed for {}",
                c.param("password").unwrap_or_default()
            )))
        }
    }

    #[test]
    fn saved_passwords_do_not_reach_the_agent() {
        let engine = Engine::new(Store::in_memory())
            .with_secrets(Arc::new(crate::secrets::MemorySecrets::default()));
        engine.register_driver(Arc::new(NullDriver));
        let s = McpServer::new(Arc::new(engine));
        let saved = s
            .engine
            .save_connection(ConnectionConfig {
                name: "prod".into(),
                driver: "null".into(),
                params: [("password".to_string(), "s3cr3t-pw".to_string())].into(),
                ..Default::default()
            })
            .unwrap();
        // A second one whose password is only in the keychain (as after a restart).
        let secrets = Arc::new(crate::secrets::MemorySecrets::default());
        let engine2 = Engine::new(Store::in_memory()).with_secrets(secrets.clone());
        engine2.register_driver(Arc::new(NullDriver));
        let kc = engine2
            .save_connection(ConnectionConfig {
                name: "kc".into(),
                driver: "null".into(),
                ..Default::default()
            })
            .unwrap();
        use crate::secrets::SecretStore as _;
        secrets.set(&kc.id, "password", "k3ychain-only").unwrap();
        let s2 = McpServer::new(Arc::new(engine2));
        let r = call(&s2, 8, "tools/call", json!({ "name": "query", "arguments": { "connection": "kc", "sql": "select 1" } }));
        assert!(r.to_string().contains("login failed"), "{r}");
        assert!(!r.to_string().contains("k3ychain-only"), "keychain secret leaked: {r}");
        // The store really holds it in memory: what the redaction has to catch.
        assert!(serde_json::to_string(&s.engine.connections()).unwrap().contains("s3cr3t-pw"));
        let attempts = [
            ("run_command", json!({ "command": "connection.list" })),
            ("run_command", json!({ "command": "connection.parseUrl", "params": { "url": "postgres://u:s3cr3t-pw@h/d" } })),
            ("run_command", json!({ "command": "connection.save", "params": { "id": saved.id, "name": "prod", "driver": "null", "params": { "password": "s3cr3t-pw" } } })),
            ("list_connections", json!({})),
            ("query", json!({ "connection": "prod", "sql": "select 1" })),
            ("query", json!({ "connection": "postgres://u:s3cr3t-pw@h/d", "sql": "select 1" })),
        ];
        for (tool, args) in attempts {
            let r = call(&s, 9, "tools/call", json!({ "name": tool, "arguments": args }));
            assert!(!r.to_string().contains("s3cr3t-pw"), "{tool} leaked: {r}");
        }
    }

    #[test]
    fn general_commands_run_without_a_connection() {
        let s = server();
        let v = s
            .call(
                "run_command",
                &json!({ "command": "query.params", "params": { "sql": "select :a" } }),
            )
            .unwrap();
        assert_eq!(v, json!(["a"]));
        let v = s.call("list_connections", &json!({})).unwrap();
        assert_eq!(v["connections"], json!([]));
    }
}
