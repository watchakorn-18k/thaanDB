//! The driver interface every database backend implements.

use crate::connection::ConnectionConfig;
use crate::dialect::Dialect;
use crate::schema::{ObjectRef, SchemaObject, TableInfo};
use crate::table::{PageRequest, StagedChange};
use crate::{Result, ResultSet, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParamKind {
    Text,
    Password,
    Number,
    /// A path on disk; the GUI shows a file picker.
    File,
    Bool,
    /// One of `choices`.
    Choice,
}

/// One field of a connection form. The GUI generates the form from these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    pub kind: ParamKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub choices: Vec<String>,
}

impl ParamSpec {
    pub fn new(key: &str, label: &str, kind: ParamKind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind,
            required: false,
            default: None,
            placeholder: None,
            choices: Vec::new(),
        }
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn default(mut self, v: &str) -> Self {
        self.default = Some(v.into());
        self
    }
    pub fn placeholder(mut self, v: &str) -> Self {
        self.placeholder = Some(v.into());
        self
    }
    pub fn choices(mut self, c: &[&str]) -> Self {
        self.choices = c.iter().map(|s| s.to_string()).collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverInfo {
    pub id: String,
    pub name: String,
    pub params: Vec<ParamSpec>,
}

pub trait Driver: Send + Sync {
    fn info(&self) -> DriverInfo;
    fn dialect(&self) -> Dialect;
    fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>>;
}

/// An open connection. Calls may block; the GUI runs them on a background executor.
pub trait Session: Send {
    /// Runs one statement with bound parameters (already in the driver's placeholder style).
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<ResultSet>;
    fn list_objects(&mut self) -> Result<Vec<SchemaObject>>;
    fn describe_table(&mut self, table: &ObjectRef) -> Result<TableInfo>;
    /// `CREATE …` text for an object.
    fn ddl(&mut self, object: &ObjectRef) -> Result<String>;
    fn begin(&mut self) -> Result<()> {
        self.execute("BEGIN", &[]).map(|_| ())
    }
    fn commit(&mut self) -> Result<()> {
        self.execute("COMMIT", &[]).map(|_| ())
    }
    fn rollback(&mut self) -> Result<()> {
        self.execute("ROLLBACK", &[]).map(|_| ())
    }

    /// Cheap round trip used by "Test connection".
    fn ping(&mut self) -> Result<()> {
        self.execute("SELECT 1", &[]).map(|_| ())
    }

    // Hooks for drivers whose tables are not SQL (document stores). SQL drivers keep the
    // defaults and the engine generates SQL through their `Dialect`.

    /// Rows of a table plus the total matching count. `paged == false` returns every
    /// matching row (export). `None` means "use the SQL path".
    fn fetch_page(&mut self, _req: &PageRequest, _paged: bool) -> Option<Result<(ResultSet, u64)>> {
        None
    }

    /// Applies staged edits; returns affected rows. `None` means "use the SQL path".
    fn apply_changes(&mut self, _table: &ObjectRef, _changes: &[StagedChange]) -> Option<Result<u64>> {
        None
    }

    /// Human-readable preview of staged edits. `None` means "use the SQL path".
    fn preview_changes(&mut self, _table: &ObjectRef, _changes: &[StagedChange]) -> Option<Vec<String>> {
        None
    }
}
