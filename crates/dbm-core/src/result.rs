use crate::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// Declared type as reported by the driver, if any.
    #[serde(default)]
    pub type_name: Option<String>,
}

impl Column {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_name: None,
        }
    }
}

/// The result of one statement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    /// Rows changed by a write statement, when the driver reports it.
    #[serde(default)]
    pub rows_affected: Option<u64>,
    /// Wall-clock duration of the statement.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// The statement that produced this result.
    #[serde(default)]
    pub sql: String,
}

impl ResultSet {
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// Rows as JSON objects keyed by column name.
    pub fn to_json_rows(&self) -> Vec<serde_json::Value> {
        self.rows
            .iter()
            .map(|row| {
                let mut obj = serde_json::Map::new();
                for (c, v) in self.columns.iter().zip(row) {
                    obj.insert(c.name.clone(), v.to_json());
                }
                serde_json::Value::Object(obj)
            })
            .collect()
    }
}
