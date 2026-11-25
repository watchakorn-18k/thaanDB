use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Index,
    Trigger,
    Routine,
    Sequence,
}

impl ObjectKind {
    pub fn label_plural(self) -> &'static str {
        match self {
            ObjectKind::Table => "Tables",
            ObjectKind::View => "Views",
            ObjectKind::MaterializedView => "Materialized views",
            ObjectKind::Index => "Indexes",
            ObjectKind::Trigger => "Triggers",
            ObjectKind::Routine => "Routines",
            ObjectKind::Sequence => "Sequences",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectRef {
    #[serde(default)]
    pub schema: Option<String>,
    pub name: String,
}

impl ObjectRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaObject {
    #[serde(flatten)]
    pub object: ObjectRef,
    pub kind: ObjectKind,
    /// For indexes and triggers: the table they belong to.
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub default: Option<String>,
    /// 1-based position in the primary key, if part of it.
    #[serde(default)]
    pub primary_key: Option<u32>,
    /// The column's comment, where the database keeps one (not SQLite).
    #[serde(default)]
    pub comment: Option<String>,
    /// MySQL `AUTO_INCREMENT` (PostgreSQL shows a `nextval(…)` default instead).
    #[serde(default)]
    pub auto_increment: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    /// Constraint name (SQLite foreign keys are often unnamed).
    #[serde(default)]
    pub name: Option<String>,
    pub columns: Vec<String>,
    /// Schema of the referenced table, when the database has schemas.
    #[serde(default)]
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    /// Referential actions as the database names them (`NO ACTION`, `CASCADE`…).
    #[serde(default)]
    pub on_update: Option<String>,
    #[serde(default)]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    /// The index behind the primary key.
    #[serde(default)]
    pub primary: bool,
}

/// A trigger on a table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerInfo {
    pub name: String,
    /// `BEFORE`, `AFTER` or `INSTEAD OF`.
    pub timing: String,
    /// `INSERT`, `UPDATE`, `DELETE` (several joined with ` OR `).
    pub event: String,
    /// What it runs: the statement body, or `EXECUTE FUNCTION f()`.
    pub action: String,
    /// The `WHEN` condition, if any.
    #[serde(default)]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    #[serde(flatten)]
    pub object: ObjectRef,
    pub columns: Vec<ColumnInfo>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
    #[serde(default)]
    pub indexes: Vec<IndexInfo>,
    #[serde(default)]
    pub triggers: Vec<TriggerInfo>,
}

impl TableInfo {
    /// Primary-key columns in key order.
    pub fn primary_key(&self) -> Vec<&str> {
        let mut pk: Vec<_> = self
            .columns
            .iter()
            .filter_map(|c| c.primary_key.map(|p| (p, c.name.as_str())))
            .collect();
        pk.sort();
        pk.into_iter().map(|(_, n)| n).collect()
    }
}
