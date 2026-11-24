use serde::{Deserialize, Serialize};
use std::fmt;

/// A single database value, independent of driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Converts to a JSON value (bytes become a hex string).
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => (*b).into(),
            Value::Int(i) => (*i).into(),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Text(s) => s.clone().into(),
            Value::Bytes(b) => format!("0x{}", hex(b)).into(),
        }
    }

    /// Converts a plain JSON value (as typed by a user or sent by MCP) into a `Value`.
    pub fn from_json(v: &serde_json::Value) -> Value {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => n
                .as_i64()
                .map(Value::Int)
                .unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0))),
            serde_json::Value::String(s) => Value::Text(s.clone()),
            other => Value::Text(other.to_string()),
        }
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("NULL"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Text(s) => f.write_str(s),
            Value::Bytes(b) => write!(f, "0x{}", hex(b)),
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}
impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}
impl From<f64> for Value {
    fn from(x: f64) -> Self {
        Value::Float(x)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
