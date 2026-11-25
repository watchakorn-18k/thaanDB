//! The engine: drivers, saved data and open sessions. Shared by `Arc<Engine>`; every
//! method takes `&self` and locks only what it needs, so a long query on one session
//! does not block another.
//!
//! Store and session guards are held on purpose: through `save()` so the file matches the
//! data, and from `begin()` to `commit()` so no other caller runs inside the transaction.
#![allow(clippy::significant_drop_tightening)]

use crate::connection::ConnectionConfig;
use crate::dialect::{Dialect, QueryLanguage};
use crate::driver::{Driver, DriverInfo, Session};
use crate::export::{self, ExportOptions};
use crate::params;
use crate::schema::{ObjectRef, SchemaObject, TableInfo};
use crate::secrets::SecretStore;
use crate::splitter::{self, Statement};
use crate::store::{new_id, now_ms, HistoryEntry, SavedQuery, Store, SECRET_KEYS};
use crate::table::{self, PageRequest, StagedChange};
use crate::{Error, Result, ResultSet, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

pub type SessionId = u64;

pub struct OpenSession {
    pub id: SessionId,
    pub connection: ConnectionConfig,
    pub dialect: Dialect,
    inner: Mutex<Box<dyn Session>>,
}

impl OpenSession {
    fn lock(&self) -> MutexGuard<'_, Box<dyn Session>> {
        // A panic in a driver poisons only that session; keep using it.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseList {
    pub current: Option<String>,
    pub databases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub connection_id: String,
    pub name: String,
    pub driver: String,
    pub read_only: bool,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Which part of the editor buffer to run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RunScope {
    #[default]
    All,
    /// The statement at this byte offset.
    Current { cursor: usize },
    /// A byte range (selection).
    Range { start: usize, end: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub session_id: SessionId,
    pub sql: String,
    #[serde(default)]
    pub scope: RunScope,
    /// Values for `:name` / `$1` / `?` placeholders, keyed by `Placeholder::key`.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub sql: String,
    #[serde(default)]
    pub result: Option<ResultSet>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePage {
    pub result: ResultSet,
    pub total_rows: u64,
    pub page: u64,
    pub page_size: u64,
}

impl TablePage {
    pub fn page_count(&self) -> u64 {
        self.total_rows.div_ceil(self.page_size.max(1)).max(1)
    }
}

pub struct Engine {
    drivers: Mutex<Vec<Arc<dyn Driver>>>,
    store: Mutex<Store>,
    sessions: Mutex<HashMap<SessionId, Arc<OpenSession>>>,
    next_session: AtomicU64,
    /// Keeps passwords between runs (the OS keychain in the app); `None` keeps them in
    /// memory only.
    secrets: Option<Arc<dyn SecretStore>>,
    /// What this run already read from or wrote to `secrets` (misses too), so the OS
    /// keychain is asked at most once per connection and key per run: each read can
    /// show a system prompt.
    secret_cache: Mutex<HashMap<(String, String), Option<String>>>,
}

impl Engine {
    pub fn new(store: Store) -> Self {
        Self {
            drivers: Mutex::new(Vec::new()),
            store: Mutex::new(store),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            secrets: None,
            secret_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Saves connection secrets in `secrets` and reads them back when a saved
    /// connection opens without them.
    pub fn with_secrets(mut self, secrets: Arc<dyn SecretStore>) -> Self {
        self.secrets = Some(secrets);
        self
    }

    /// The stored secret `key` of saved connection `connection_id`, if any. Can block
    /// on the OS keychain; call it off the UI thread.
    pub fn secret(&self, connection_id: &str, key: &str) -> Option<String> {
        if connection_id.is_empty() {
            return None;
        }
        let store = self.secrets.as_ref()?;
        let slot = (connection_id.to_string(), key.to_string());
        if let Some(v) = self.secret_cache().get(&slot) {
            return v.clone();
        }
        let v = store.get(connection_id, key);
        self.secret_cache().insert(slot, v.clone());
        v
    }

    fn secret_cache(&self) -> MutexGuard<'_, HashMap<(String, String), Option<String>>> {
        self.secret_cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Writes a secret to the store and the run's cache.
    fn put_secret(&self, connection_id: &str, key: &str, value: &str) -> Result<()> {
        let Some(store) = &self.secrets else {
            return Ok(());
        };
        store.set(connection_id, key, value)?;
        self.secret_cache().insert(
            (connection_id.to_string(), key.to_string()),
            Some(value.to_string()),
        );
        Ok(())
    }

    /// Fills secrets a saved connection is missing (an empty password box means "use
    /// the stored one"). Returns the keys the caller typed itself.
    fn fill_secrets(&self, c: &mut ConnectionConfig) -> Vec<&'static str> {
        let mut typed = Vec::new();
        for k in SECRET_KEYS {
            if c.param(k).is_some() {
                typed.push(*k);
            } else if let Some(v) = self.secret(&c.id, k) {
                c.params.insert((*k).to_string(), v);
            }
        }
        typed
    }

    /// After a successful connect: remembers secrets typed for a saved connection, so
    /// the next run does not ask again.
    fn remember_secrets(&self, c: &ConnectionConfig, typed: &[&str]) {
        if self.secrets.is_none()
            || typed.is_empty()
            || !self.store().data.connections.iter().any(|x| x.id == c.id)
        {
            return;
        }
        for k in typed {
            if let Some(v) = c.param(k) {
                if self.secret(&c.id, k).as_deref() != Some(v) {
                    self.put_secret(&c.id, k, v).ok();
                }
            }
        }
    }

    pub fn register_driver(&self, d: Arc<dyn Driver>) {
        self.drivers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(d);
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn driver(&self, id: &str) -> Result<Arc<dyn Driver>> {
        self.drivers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|d| d.info().id == id)
            .cloned()
            .ok_or_else(|| Error::UnknownDriver(id.into()))
    }

    pub fn session(&self, id: SessionId) -> Result<Arc<OpenSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
            .ok_or(Error::UnknownSession(id))
    }

    // ---- drivers & connections ------------------------------------------------------

    pub fn drivers(&self) -> Vec<DriverInfo> {
        self.drivers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|d| d.info())
            .collect()
    }

    pub fn connections(&self) -> Vec<ConnectionConfig> {
        self.store().data.connections.clone()
    }

    pub fn connection(&self, id: &str) -> Result<ConnectionConfig> {
        self.store()
            .data
            .connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| Error::UnknownConnection(id.into()))
    }

    pub fn save_connection(&self, mut c: ConnectionConfig) -> Result<ConnectionConfig> {
        self.driver(&c.driver)?;
        if c.name.trim().is_empty() {
            return Err(Error::InvalidParams("connection name is required".into()));
        }
        if c.id.is_empty() {
            c.id = new_id();
        }
        {
            let mut s = self.store();
            match s.data.connections.iter_mut().find(|x| x.id == c.id) {
                Some(x) => *x = c.clone(),
                None => s.data.connections.push(c.clone()),
            }
            s.save()?;
        }
        // A secret typed in the form goes to the keychain; an empty one keeps what is
        // stored, so editing other fields never drops a saved password.
        for k in SECRET_KEYS {
            if let Some(v) = c.param(k) {
                // Unchanged since this run read or wrote it: no keychain call at all.
                if self
                    .secret_cache()
                    .get(&(c.id.clone(), (*k).to_string()))
                    .cloned()
                    .flatten()
                    .as_deref()
                    != Some(v)
                {
                    self.put_secret(&c.id, k, v)?;
                }
            }
        }
        Ok(c)
    }

    pub fn delete_connection(&self, id: &str) -> Result<()> {
        let mut s = self.store();
        let before = s.data.connections.len();
        s.data.connections.retain(|c| c.id != id);
        if s.data.connections.len() == before {
            return Err(Error::UnknownConnection(id.into()));
        }
        s.save()?;
        drop(s);
        if let Some(store) = &self.secrets {
            for k in SECRET_KEYS {
                store.delete(id, k);
                self.secret_cache()
                    .insert((id.to_string(), (*k).to_string()), None);
            }
        }
        Ok(())
    }

    // ---- connection groups ------------------------------------------------------------

    /// Groups in display order: the stored list, then any group a connection names that the
    /// list does not have yet (older stores, hand-edited files).
    pub fn folders(&self) -> Vec<String> {
        let s = self.store();
        let mut out = s.data.folders.clone();
        for c in &s.data.connections {
            if let Some(f) = &c.folder {
                if !out.iter().any(|x| x == f) {
                    out.push(f.clone());
                }
            }
        }
        out
    }

    /// Adds an empty group at the end. Names are trimmed and must be unique (ignoring case).
    pub fn create_folder(&self, name: &str) -> Result<Vec<String>> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::InvalidParams("group name is required".into()));
        }
        if self
            .folders()
            .iter()
            .any(|f| f.to_lowercase() == name.to_lowercase())
        {
            return Err(Error::InvalidParams(format!(
                "a group named “{name}” exists"
            )));
        }
        let mut s = self.store();
        s.data.folders.push(name.to_string());
        s.save()?;
        drop(s);
        Ok(self.folders())
    }

    /// Removes a group. Its connections stay, outside any group.
    pub fn delete_folder(&self, name: &str) -> Result<Vec<String>> {
        let mut s = self.store();
        let had = s.data.folders.iter().any(|f| f == name)
            || s.data
                .connections
                .iter()
                .any(|c| c.folder.as_deref() == Some(name));
        if !had {
            return Err(Error::InvalidParams(format!("no group named “{name}”")));
        }
        s.data.folders.retain(|f| f != name);
        for c in &mut s.data.connections {
            if c.folder.as_deref() == Some(name) {
                c.folder = None;
            }
        }
        s.save()?;
        drop(s);
        Ok(self.folders())
    }

    /// Puts a connection in a group (created if new), or takes it out with `None`.
    pub fn move_connection(&self, id: &str, folder: Option<&str>) -> Result<ConnectionConfig> {
        let folder = folder.map(str::trim).filter(|f| !f.is_empty());
        let mut s = self.store();
        if let Some(f) = folder {
            if !s.data.folders.iter().any(|x| x == f) {
                s.data.folders.push(f.to_string());
            }
        }
        let c = s
            .data
            .connections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::UnknownConnection(id.into()))?;
        c.folder = folder.map(String::from);
        let out = c.clone();
        s.save()?;
        Ok(out)
    }

    /// Moves a connection into `folder` (`None`: no group) and puts it just before the
    /// connection `before`, or last when `before` is `None` or unknown. Drag and drop in the
    /// sidebar uses this; the list order is the saved order.
    pub fn place_connection(
        &self,
        id: &str,
        folder: Option<&str>,
        before: Option<&str>,
    ) -> Result<ConnectionConfig> {
        let folder = folder.map(str::trim).filter(|f| !f.is_empty());
        let mut s = self.store();
        let from = s
            .data
            .connections
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| Error::UnknownConnection(id.into()))?;
        let mut c = s.data.connections.remove(from);
        c.folder = folder.map(String::from);
        let to = before
            .filter(|b| *b != id)
            .and_then(|b| s.data.connections.iter().position(|x| x.id == b))
            .unwrap_or(s.data.connections.len());
        s.data.connections.insert(to, c.clone());
        if let Some(f) = folder {
            if !s.data.folders.iter().any(|x| x == f) {
                s.data.folders.push(f.to_string());
            }
        }
        s.save()?;
        Ok(c)
    }

    pub fn test_connection(&self, c: &ConnectionConfig) -> TestResult {
        let t = Instant::now();
        let mut c = c.clone();
        self.fill_secrets(&mut c);
        let r = self
            .driver(&c.driver)
            .and_then(|d| d.connect(&c))
            .and_then(|mut s| s.ping());
        TestResult {
            ok: r.is_ok(),
            elapsed_ms: t.elapsed().as_millis() as u64,
            error: r.err().map(|e| e.to_string()),
        }
    }

    // ---- sessions ---------------------------------------------------------------------

    /// Opens a session from a full config (it does not need to be saved).
    pub fn open(&self, mut c: ConnectionConfig) -> Result<SessionInfo> {
        let driver = self.driver(&c.driver)?;
        let typed = self.fill_secrets(&mut c);
        let inner = driver.connect(&c)?;
        self.remember_secrets(&c, &typed);
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let info = SessionInfo {
            session_id: id,
            connection_id: c.id.clone(),
            name: c.name.clone(),
            driver: c.driver.clone(),
            read_only: c.read_only,
            color: c.color.clone(),
        };
        let s = OpenSession {
            id,
            connection: c,
            dialect: driver.dialect(),
            inner: Mutex::new(inner),
        };
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, Arc::new(s));
        Ok(info)
    }

    pub fn open_saved(&self, connection_id: &str) -> Result<SessionInfo> {
        self.open(self.connection(connection_id)?)
    }

    pub fn close(&self, id: SessionId) -> Result<()> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id)
            .map(|_| ())
            .ok_or(Error::UnknownSession(id))
    }

    pub fn sessions(&self) -> Vec<SessionInfo> {
        let mut v: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|s| SessionInfo {
                session_id: s.id,
                connection_id: s.connection.id.clone(),
                name: s.connection.name.clone(),
                driver: s.connection.driver.clone(),
                read_only: s.connection.read_only,
                color: s.connection.color.clone(),
            })
            .collect();
        v.sort_by_key(|s| s.session_id);
        v
    }

    // ---- queries ----------------------------------------------------------------------

    pub fn split(&self, session_id: SessionId, sql: &str) -> Result<Vec<Statement>> {
        Ok(splitter::split(
            sql,
            self.session(session_id)?.dialect.split,
        ))
    }

    /// Runs the statements selected by `scope`. Stops at the first failing statement.
    pub fn run_query(&self, req: &QueryRequest) -> Result<Vec<StatementResult>> {
        let s = self.session(req.session_id)?;
        let all = splitter::split(&req.sql, s.dialect.split);
        let chosen: Vec<Statement> = match req.scope {
            RunScope::All => all,
            RunScope::Current { cursor } => splitter::statement_at(&all, cursor)
                .cloned()
                .into_iter()
                .collect(),
            RunScope::Range { start, end } => {
                let start = start.min(req.sql.len());
                let end = end.clamp(start, req.sql.len());
                let text = req.sql.get(start..end).unwrap_or_default();
                splitter::split(text, s.dialect.split)
            }
        };
        let mut out = Vec::new();
        let mut session = s.lock();
        for st in chosen {
            let t = Instant::now();
            let r = self.exec_one(&s, session.as_mut(), &st.sql, &req.params);
            let elapsed = t.elapsed().as_millis() as u64;
            let (result, error) = match r {
                Ok(mut rs) => {
                    rs.elapsed_ms = elapsed;
                    rs.sql = st.sql.clone();
                    (Some(rs), None)
                }
                Err(e) => (None, Some(e.to_string())),
            };
            self.store().push_history(HistoryEntry {
                id: String::new(),
                connection_id: s.connection.id.clone(),
                sql: st.sql.clone(),
                at_ms: now_ms(),
                elapsed_ms: elapsed,
                rows: result
                    .as_ref()
                    .map(|r| r.rows_affected.unwrap_or(r.rows.len() as u64)),
                error: error.clone(),
            });
            let failed = error.is_some();
            out.push(StatementResult {
                sql: st.sql,
                result,
                error,
            });
            if failed {
                break;
            }
        }
        drop(session);
        self.store().save()?;
        Ok(out)
    }

    fn exec_one(
        &self,
        s: &OpenSession,
        session: &mut dyn Session,
        sql: &str,
        values: &BTreeMap<String, Value>,
    ) -> Result<ResultSet> {
        if s.dialect.language != QueryLanguage::Sql {
            // Non-SQL drivers parse their own language and enforce read-only themselves.
            return session.execute(sql, &[]);
        }
        if s.connection.read_only && !is_read_statement(sql) {
            return Err(Error::ReadOnly(first_word(sql).to_uppercase()));
        }
        let (sql, bound) = params::bind(sql, values, s.dialect.placeholders)?;
        session.execute(&sql, &bound)
    }

    /// Runs one statement or driver command without adding it to history: for views
    /// that poll or act on the user's behalf (the broker console, the key browser).
    /// Read-only connections refuse writes as in `run_query`.
    pub fn driver_command(&self, id: SessionId, text: &str) -> Result<ResultSet> {
        let s = self.session(id)?;
        let mut sess = s.lock();
        let t = Instant::now();
        let mut rs = self.exec_one(&s, sess.as_mut(), text, &BTreeMap::new())?;
        rs.elapsed_ms = t.elapsed().as_millis() as u64;
        rs.sql = text.to_string();
        Ok(rs)
    }

    // ---- key-value and broker views ---------------------------------------------------

    pub fn scan_keys(
        &self,
        id: SessionId,
        scan: &crate::browse::KeyScan,
    ) -> Result<crate::browse::KeyPage> {
        self.session(id)?
            .lock()
            .scan_keys(scan)
            .unwrap_or_else(|| Err(Error::Unsupported("key browsing on this connection".into())))
    }

    pub fn messages_since(
        &self,
        id: SessionId,
        since: u64,
        limit: usize,
    ) -> Result<Vec<crate::browse::BrokerMessage>> {
        self.session(id)?
            .lock()
            .messages_since(since, limit)
            .unwrap_or_else(|| Err(Error::Unsupported("messages on this connection".into())))
    }

    pub fn subscriptions(&self, id: SessionId) -> Result<Vec<crate::browse::Subscription>> {
        self.session(id)?.lock().subscriptions().unwrap_or_else(|| {
            Err(Error::Unsupported(
                "subscriptions on this connection".into(),
            ))
        })
    }

    // ---- schema -----------------------------------------------------------------------

    pub fn list_objects(&self, id: SessionId) -> Result<Vec<SchemaObject>> {
        self.session(id)?.lock().list_objects()
    }

    /// The server's databases and the one this session uses.
    pub fn list_databases(&self, id: SessionId) -> Result<DatabaseList> {
        let s = self.session(id)?;
        let mut sess = s.lock();
        let current = sess.current_database()?;
        let mut databases = sess.list_databases()?;
        if let Some(c) = &current {
            if !databases.contains(c) {
                databases.push(c.clone());
                databases.sort();
            }
        }
        Ok(DatabaseList { current, databases })
    }

    /// Objects of any database on the server. Other databases are read through a short
    /// read-only connection of their own, since some servers (PostgreSQL) cannot look
    /// across databases from one connection.
    pub fn list_objects_in(&self, id: SessionId, database: &str) -> Result<Vec<SchemaObject>> {
        let s = self.session(id)?;
        if s.lock().current_database()?.as_deref() == Some(database) {
            return s.lock().list_objects();
        }
        let mut cfg = s.connection.clone();
        cfg.params.insert("database".into(), database.to_string());
        cfg.read_only = true;
        let mut tmp = self.driver(&cfg.driver)?.connect(&cfg)?;
        tmp.list_objects()
    }

    /// Reopens the session on another database of the same server. The old session is
    /// closed; the returned session replaces it.
    pub fn switch_database(&self, id: SessionId, database: &str) -> Result<SessionInfo> {
        let info = self.open_database(id, database)?;
        self.close(id).ok();
        Ok(info)
    }

    /// Opens another session on the same server, on `database`; session `id` stays open.
    /// Lets one connection show tables of several databases side by side.
    pub fn open_database(&self, id: SessionId, database: &str) -> Result<SessionInfo> {
        let mut cfg = self.session(id)?.connection.clone();
        // Names made from a URL read `db@host`; keep them truthful. Typed names stay.
        if let Some(old) = cfg.params.get("database") {
            if let Some(rest) = cfg.name.strip_prefix(&format!("{old}@")) {
                cfg.name = format!("{database}@{rest}");
            }
        }
        cfg.params.insert("database".into(), database.to_string());
        self.open(cfg)
    }

    /// (read-only, dialect) of an open session.
    pub(crate) fn session_info_of(&self, id: SessionId) -> Result<(bool, Dialect)> {
        let s = self.session(id)?;
        Ok((s.connection.read_only, s.dialect))
    }

    /// The exact statements `run_object_op` would run, for a preview before confirming.
    pub fn object_op_statements(
        &self,
        id: SessionId,
        kind: crate::schema::ObjectKind,
        object: &ObjectRef,
        op: &crate::object_ops::ObjectOp,
    ) -> Result<Vec<String>> {
        let s = self.session(id)?;
        crate::object_ops::statements(&s.dialect, kind, object, op)
    }

    /// The exact statements `run_structure_op` would run, for the preview.
    pub fn structure_op_statements(
        &self,
        id: SessionId,
        table: &ObjectRef,
        op: &crate::structure_ops::StructureOp,
    ) -> Result<Vec<String>> {
        let s = self.session(id)?;
        crate::structure_ops::statements(&s.dialect, table, op)
    }

    /// Adds or drops a column, index or foreign key of a SQL table. Refused on read-only
    /// connections; statements run in order, stop at the first error and go to history.
    pub fn run_structure_op(
        &self,
        id: SessionId,
        table: &ObjectRef,
        op: &crate::structure_ops::StructureOp,
    ) -> Result<()> {
        let (read_only, dialect) = {
            let s = self.session(id)?;
            (s.connection.read_only, s.dialect)
        };
        if read_only {
            return Err(Error::ReadOnly(format!("{} on {}", op.verb(), table.name)));
        }
        let statements = self.structure_op_statements(id, table, op)?;
        let run = |sql: String| -> Result<()> {
            let results = self.run_query(&QueryRequest {
                session_id: id,
                sql,
                scope: RunScope::All,
                params: BTreeMap::new(),
            })?;
            match results.into_iter().find_map(|r| r.error) {
                Some(e) => Err(Error::Database(e)),
                None => Ok(()),
            }
        };
        // PostgreSQL and SQLite roll DDL back, so a change of several statements (a
        // rename plus a new type) lands whole or not at all. MySQL commits each one.
        let atomic = statements.len() > 1 && matches!(dialect.name, "postgres" | "sqlite");
        if atomic {
            run("BEGIN".into())?;
        }
        for sql in statements {
            if let Err(e) = run(sql) {
                if atomic {
                    run("ROLLBACK".into()).ok();
                }
                return Err(e);
            }
        }
        if atomic {
            run("COMMIT".into())?;
        }
        Ok(())
    }

    /// Renames, drops, truncates or duplicates a table, view or collection. Refused on
    /// read-only connections. Statements run in order and stop at the first error; each
    /// is recorded in history like a typed query.
    pub fn run_object_op(
        &self,
        id: SessionId,
        kind: crate::schema::ObjectKind,
        object: &ObjectRef,
        op: &crate::object_ops::ObjectOp,
    ) -> Result<()> {
        if self.session(id)?.connection.read_only {
            return Err(Error::ReadOnly(format!("{} {}", op.verb(), object.name)));
        }
        for sql in self.object_op_statements(id, kind, object, op)? {
            let results = self.run_query(&QueryRequest {
                session_id: id,
                sql,
                scope: RunScope::All,
                params: BTreeMap::new(),
            })?;
            if let Some(e) = results.into_iter().find_map(|r| r.error) {
                return Err(Error::Database(e));
            }
        }
        Ok(())
    }

    pub fn describe_table(&self, id: SessionId, t: &ObjectRef) -> Result<TableInfo> {
        self.session(id)?.lock().describe_table(t)
    }

    pub fn ddl(&self, id: SessionId, t: &ObjectRef) -> Result<String> {
        self.session(id)?.lock().ddl(t)
    }

    // ---- table data -------------------------------------------------------------------

    pub fn fetch_page(&self, id: SessionId, req: &PageRequest) -> Result<TablePage> {
        let s = self.session(id)?;
        let mut sess = s.lock();
        let t = Instant::now();
        if let Some(r) = sess.fetch_page(req, true) {
            let (mut result, total_rows) = r?;
            result.elapsed_ms = t.elapsed().as_millis() as u64;
            return Ok(TablePage {
                result,
                total_rows,
                page: req.page,
                page_size: req.page_size,
            });
        }
        let (page, count) = table::page_queries(&s.dialect, req);
        let mut result = sess.execute(&page.sql, &page.params)?;
        result.elapsed_ms = t.elapsed().as_millis() as u64;
        result.sql = page.sql;
        let total = sess.execute(&count.sql, &count.params)?;
        let total_rows = match total.rows.first().and_then(|r| r.first()) {
            Some(Value::Int(n)) => (*n).max(0) as u64,
            Some(Value::Text(s)) => s.parse().unwrap_or(0),
            _ => 0,
        };
        Ok(TablePage {
            result,
            total_rows,
            page: req.page,
            page_size: req.page_size,
        })
    }

    pub fn preview_changes(
        &self,
        id: SessionId,
        t: &ObjectRef,
        changes: &[StagedChange],
    ) -> Result<Vec<String>> {
        let s = self.session(id)?;
        let native = s.lock().preview_changes(t, changes);
        if let Some(p) = native {
            return Ok(p);
        }
        Ok(table::change_statements(&s.dialect, t, changes)
            .iter()
            .map(|b| b.preview(&s.dialect))
            .collect())
    }

    /// Applies staged changes in one transaction. Returns the number of affected rows.
    pub fn apply_changes(
        &self,
        id: SessionId,
        t: &ObjectRef,
        changes: &[StagedChange],
    ) -> Result<u64> {
        let s = self.session(id)?;
        if s.connection.read_only {
            return Err(Error::ReadOnly("staged changes".into()));
        }
        let mut sess = s.lock();
        if let Some(r) = sess.apply_changes(t, changes) {
            return r;
        }
        let stmts = table::change_statements(&s.dialect, t, changes);
        sess.begin()?;
        let mut affected = 0;
        for st in &stmts {
            match sess.execute(&st.sql, &st.params) {
                Ok(r) => affected += r.rows_affected.unwrap_or(0),
                Err(e) => {
                    sess.rollback().ok();
                    return Err(e);
                }
            }
        }
        sess.commit()?;
        Ok(affected)
    }

    // ---- export -----------------------------------------------------------------------

    /// Exports every row of a table (filters and sort honoured, no paging).
    pub fn export_table(
        &self,
        id: SessionId,
        req: &PageRequest,
        opts: &ExportOptions,
    ) -> Result<String> {
        let s = self.session(id)?;
        let mut sess = s.lock();
        let rs = match sess.fetch_page(req, false) {
            Some(r) => r?.0,
            None => {
                let q = table::select_all(&s.dialect, req);
                sess.execute(&q.sql, &q.params)?
            }
        };
        drop(sess);
        let mut opts = opts.clone();
        if opts.table_name.is_none() {
            opts.table_name = Some(req.table.name.clone());
        }
        Ok(export::export(&rs, &opts, &s.dialect))
    }

    pub fn export_result(
        &self,
        id: SessionId,
        rs: &ResultSet,
        opts: &ExportOptions,
    ) -> Result<String> {
        Ok(export::export(rs, opts, &self.session(id)?.dialect))
    }

    // ---- saved queries & history ------------------------------------------------------

    pub fn saved_queries(&self) -> Vec<SavedQuery> {
        self.store().data.saved_queries.clone()
    }

    pub fn save_query(&self, mut q: SavedQuery) -> Result<SavedQuery> {
        if q.id.is_empty() {
            q.id = new_id();
        }
        q.updated_ms = now_ms();
        let mut s = self.store();
        match s.data.saved_queries.iter_mut().find(|x| x.id == q.id) {
            Some(x) => *x = q.clone(),
            None => s.data.saved_queries.push(q.clone()),
        }
        s.save()?;
        Ok(q)
    }

    pub fn delete_saved_query(&self, id: &str) -> Result<()> {
        let mut s = self.store();
        s.data.saved_queries.retain(|q| q.id != id);
        s.save()
    }

    /// Newest first.
    pub fn history(&self, limit: usize) -> Vec<HistoryEntry> {
        self.store()
            .data
            .history
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn clear_history(&self) -> Result<()> {
        let mut s = self.store();
        s.data.history.clear();
        s.save()
    }

    // ---- settings ---------------------------------------------------------------------

    pub fn settings(&self) -> crate::store::Settings {
        self.store().data.settings.clone().clamped()
    }

    /// Stores new settings (out-of-range values are clamped) and trims history to the
    /// new limit. Returns what was stored. Writes the store file: call it off the UI thread.
    pub fn set_settings(&self, settings: crate::store::Settings) -> Result<crate::store::Settings> {
        let settings = settings.clamped();
        let mut s = self.store();
        s.data.settings = settings.clone();
        s.trim_history();
        s.save()?;
        Ok(settings)
    }
}

fn first_word(sql: &str) -> String {
    let mut s = sql.trim_start();
    loop {
        if let Some(r) = s.strip_prefix("--") {
            s = r.split_once('\n').map_or("", |x| x.1).trim_start();
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.split_once("*/").map_or("", |x| x.1).trim_start();
        } else if let Some(r) = s.strip_prefix('(') {
            s = r.trim_start();
        } else {
            break;
        }
    }
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Conservative check used for read-only connections. Drivers also enforce read-only at
/// the session level where the database supports it.
pub fn is_read_statement(sql: &str) -> bool {
    let w = first_word(sql);
    let read = matches!(
        w.as_str(),
        "select"
            | "with"
            | "explain"
            | "show"
            | "describe"
            | "desc"
            | "values"
            | "table"
            | "pragma"
    );
    if !read {
        return false;
    }
    if w == "with" || w == "explain" {
        // Data-modifying CTEs / EXPLAIN ANALYZE of writes.
        let lower = sql.to_ascii_lowercase();
        let writes = [
            "insert ",
            "update ",
            "delete ",
            "merge ",
            "drop ",
            "alter ",
            "truncate ",
            "create ",
        ];
        return !writes.iter().any(|k| lower.contains(k));
    }
    true
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use crate::secrets::MemorySecrets;

    /// A driver that never connects; saving only checks that the driver exists.
    struct NullDriver;
    impl Driver for NullDriver {
        fn info(&self) -> DriverInfo {
            DriverInfo {
                id: "null".into(),
                name: "Null".into(),
                params: Vec::new(),
            }
        }
        fn dialect(&self) -> Dialect {
            Dialect::for_driver("sqlite")
        }
        fn connect(&self, _: &ConnectionConfig) -> Result<Box<dyn Session>> {
            Err(Error::Unsupported("null driver".into()))
        }
    }

    fn engine(secrets: Arc<MemorySecrets>) -> Engine {
        let e = Engine::new(Store::in_memory()).with_secrets(secrets);
        e.register_driver(Arc::new(NullDriver));
        e
    }

    #[test]
    fn password_goes_to_secrets_and_comes_back() {
        let secrets = Arc::new(MemorySecrets::default());
        let e = engine(secrets.clone());
        let mut c = ConnectionConfig {
            name: "db".into(),
            driver: "null".into(),
            ..Default::default()
        };
        c.params.insert("password".into(), "s3cret".into());
        let saved = e.save_connection(c).unwrap();
        assert_eq!(
            secrets.get(&saved.id, "password").as_deref(),
            Some("s3cret")
        );

        // Editing without retyping the password keeps the stored one.
        let mut edit = saved.clone();
        edit.params.remove("password");
        edit.name = "renamed".into();
        e.save_connection(edit.clone()).unwrap();
        assert_eq!(e.secret(&saved.id, "password").as_deref(), Some("s3cret"));

        let mut filled = edit;
        e.fill_secrets(&mut filled);
        assert_eq!(filled.param("password"), Some("s3cret"));

        e.delete_connection(&saved.id).unwrap();
        assert_eq!(secrets.get(&saved.id, "password"), None);
        assert_eq!(e.secret(&saved.id, "password"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_groups() {
        let e = Engine::new(Store::in_memory());
        e.store().data.connections.push(ConnectionConfig {
            id: "c1".into(),
            name: "shop".into(),
            ..Default::default()
        });
        assert_eq!(e.create_folder("  Prod ").unwrap(), vec!["Prod"]);
        assert!(
            e.create_folder("prod").is_err(),
            "names are unique ignoring case"
        );
        assert!(e.create_folder(" ").is_err());
        // Moving into a new group creates it; moving out keeps the group.
        assert_eq!(
            e.move_connection("c1", Some("Dev"))
                .unwrap()
                .folder
                .as_deref(),
            Some("Dev")
        );
        assert_eq!(e.folders(), vec!["Prod", "Dev"]);
        assert_eq!(e.move_connection("c1", None).unwrap().folder, None);
        assert_eq!(e.folders(), vec!["Prod", "Dev"]);
        // Deleting a group frees its connections, never deletes them.
        e.move_connection("c1", Some("Prod")).unwrap();
        assert_eq!(e.delete_folder("Prod").unwrap(), vec!["Dev"]);
        assert_eq!(e.connections()[0].folder, None);
        assert_eq!(e.connections().len(), 1);
        assert!(e.delete_folder("Nope").is_err());
        assert!(e.move_connection("missing", None).is_err());
    }

    #[test]
    fn place_connection_orders_and_regroups() {
        let e = Engine::new(Store::in_memory());
        for id in ["a", "b", "c"] {
            e.store().data.connections.push(ConnectionConfig {
                id: id.into(),
                name: id.into(),
                ..Default::default()
            });
        }
        let ids = |e: &Engine| {
            e.connections()
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        // Drop "c" on "a": it goes just before "a", into a's group (none).
        e.place_connection("c", None, Some("a")).unwrap();
        assert_eq!(ids(&e), ["c", "a", "b"]);
        // Drop "c" on a group header: last, in that group, which is created.
        let c = e.place_connection("c", Some("Prod"), None).unwrap();
        assert_eq!(c.folder.as_deref(), Some("Prod"));
        assert_eq!(ids(&e), ["a", "b", "c"]);
        assert_eq!(e.folders(), ["Prod"]);
        // Dropping on itself, or before an unknown id, keeps it last.
        e.place_connection("c", Some("Prod"), Some("c")).unwrap();
        e.place_connection("c", Some("Prod"), Some("zzz")).unwrap();
        assert_eq!(ids(&e), ["a", "b", "c"]);
        assert!(e.place_connection("zzz", None, None).is_err());
    }

    #[test]
    fn read_statement_detection() {
        assert!(is_read_statement("  -- hi\n SELECT 1"));
        assert!(is_read_statement("(select 1) union (select 2)"));
        assert!(!is_read_statement("update t set a = 1"));
        assert!(!is_read_statement(
            "with x as (delete from t returning *) select * from x"
        ));
        assert!(is_read_statement("with x as (select 1) select * from x"));
    }
}

#[cfg(test)]
mod secret_cache_tests {
    use super::*;
    use crate::secrets::SecretStore;
    use std::sync::atomic::AtomicUsize;

    /// Counts reads, like a keychain that shows a prompt on each one.
    #[derive(Default)]
    struct Counting {
        reads: AtomicUsize,
    }
    impl SecretStore for Counting {
        fn get(&self, _: &str, key: &str) -> Option<String> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            (key == "password").then(|| "pw".to_string())
        }
        fn set(&self, _: &str, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _: &str, _: &str) {}
    }

    #[test]
    fn keychain_is_read_once_per_connection_and_key() {
        let store = Arc::new(Counting::default());
        let e = Engine::new(Store::in_memory()).with_secrets(store.clone());
        for _ in 0..3 {
            let mut c = ConnectionConfig {
                id: "c1".into(),
                ..Default::default()
            };
            e.fill_secrets(&mut c);
            assert_eq!(c.param("password"), Some("pw"));
        }
        // One read per secret key, however many times the connection opens.
        assert_eq!(store.reads.load(Ordering::Relaxed), SECRET_KEYS.len());
    }
}
