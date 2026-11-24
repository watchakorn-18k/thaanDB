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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
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
