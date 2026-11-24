//! The engine: drivers, saved data and open sessions. Shared by `Arc<Engine>`; every
//! method takes `&self` and locks only what it needs, so a long query on one session
//! does not block another.

use crate::connection::ConnectionConfig;
use crate::dialect::Dialect;
use crate::driver::{Driver, DriverInfo, Session};
use crate::export::{self, ExportOptions};
use crate::params;
use crate::schema::{ObjectRef, SchemaObject, TableInfo};
use crate::splitter::{self, Statement};
use crate::store::{new_id, now_ms, HistoryEntry, SavedQuery, Store};
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
}

impl Engine {
    pub fn new(store: Store) -> Self {
        Self {
            drivers: Mutex::new(Vec::new()),
            store: Mutex::new(store),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
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
        let mut s = self.store();
        match s.data.connections.iter_mut().find(|x| x.id == c.id) {
            Some(x) => *x = c.clone(),
            None => s.data.connections.push(c.clone()),
        }
        s.save()?;
        Ok(c)
    }

    pub fn delete_connection(&self, id: &str) -> Result<()> {
        let mut s = self.store();
        let before = s.data.connections.len();
        s.data.connections.retain(|c| c.id != id);
        if s.data.connections.len() == before {
            return Err(Error::UnknownConnection(id.into()));
        }
        s.save()
    }

    pub fn test_connection(&self, c: &ConnectionConfig) -> TestResult {
        let t = Instant::now();
        let r = self
            .driver(&c.driver)
            .and_then(|d| d.connect(c))
            .and_then(|mut s| {
                s.execute("SELECT 1", &[])?;
                Ok(())
            });
        TestResult {
            ok: r.is_ok(),
            elapsed_ms: t.elapsed().as_millis() as u64,
            error: r.err().map(|e| e.to_string()),
        }
    }

    // ---- sessions ---------------------------------------------------------------------

    /// Opens a session from a full config (it does not need to be saved).
    pub fn open(&self, c: ConnectionConfig) -> Result<SessionInfo> {
        let driver = self.driver(&c.driver)?;
        let inner = driver.connect(&c)?;
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
        if s.connection.read_only && !is_read_statement(sql) {
            return Err(Error::ReadOnly(first_word(sql).to_uppercase()));
        }
        let (sql, bound) = params::bind(sql, values, s.dialect.placeholders)?;
        session.execute(&sql, &bound)
    }

    // ---- schema -----------------------------------------------------------------------

    pub fn list_objects(&self, id: SessionId) -> Result<Vec<SchemaObject>> {
        self.session(id)?.lock().list_objects()
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
        let (page, count) = table::page_queries(&s.dialect, req);
        let mut sess = s.lock();
        let t = Instant::now();
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
        let stmts = table::change_statements(&s.dialect, t, changes);
        let mut sess = s.lock();
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
        let q = table::select_all(&s.dialect, req);
        let rs = s.lock().execute(&q.sql, &q.params)?;
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
}

fn first_word(sql: &str) -> String {
    let mut s = sql.trim_start();
    loop {
        if let Some(r) = s.strip_prefix("--") {
            s = r.split_once('\n').map(|x| x.1).unwrap_or("").trim_start();
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.split_once("*/").map(|x| x.1).unwrap_or("").trim_start();
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
mod tests {
    use super::*;

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
