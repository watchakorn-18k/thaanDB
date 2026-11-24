//! Typed document trees for document databases (MongoDB). The GUI's tree and JSON views
//! read these; tables keep using flattened `ResultSet::rows`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "camelCase")]
pub enum Doc {
    Null,
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Double(f64),
    /// Decimal128 as text.
    Decimal(String),
    String(String),
    /// Hex.
    ObjectId(String),
    /// RFC 3339 (UTC).
    Date(String),
    /// Shell text of a binary value, e.g. `UUID("…")` or `BinData(0, "…")`.
    Binary(String),
    /// `/pattern/flags`.
    Regex(String),
    Array(Vec<Doc>),
    Object(Vec<(String, Doc)>),
    /// Anything else (timestamp, code, min/max key…), with its type name and shell text.
    Other {
        type_name: String,
        text: String,
    },
}

/// Colour family a value is drawn with; the GUI maps these to theme tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Null,
    Bool,
    Number,
    String,
    Id,
    Date,
    Object,
    Array,
    Other,
}

impl Doc {
    pub fn type_name(&self) -> &str {
        match self {
            Doc::Null => "Null",
            Doc::Bool(_) => "Bool",
            Doc::Int32(_) => "Int32",
            Doc::Int64(_) => "Int64",
            Doc::Double(_) => "Double",
            Doc::Decimal(_) => "Decimal128",
            Doc::String(_) => "String",
            Doc::ObjectId(_) => "ObjectId",
            Doc::Date(_) => "Date",
            Doc::Binary(_) => "Binary",
            Doc::Regex(_) => "Regex",
            Doc::Array(_) => "Array",
            Doc::Object(_) => "Object",
            Doc::Other { type_name, .. } => type_name,
        }
    }

    pub fn kind(&self) -> DocKind {
        match self {
            Doc::Null => DocKind::Null,
            Doc::Bool(_) => DocKind::Bool,
            Doc::Int32(_) | Doc::Int64(_) | Doc::Double(_) | Doc::Decimal(_) => DocKind::Number,
            Doc::String(_) | Doc::Regex(_) => DocKind::String,
            Doc::ObjectId(_) | Doc::Binary(_) => DocKind::Id,
            Doc::Date(_) => DocKind::Date,
            Doc::Object(_) => DocKind::Object,
            Doc::Array(_) => DocKind::Array,
            Doc::Other { .. } => DocKind::Other,
        }
    }

    /// Field of an object by name.
    pub fn get(&self, key: &str) -> Option<&Doc> {
        match self {
            Doc::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Children for a tree view: object fields, or array items keyed by index.
    pub fn children(&self) -> Vec<(String, &Doc)> {
        match self {
            Doc::Object(f) => f.iter().map(|(k, v)| (k.clone(), v)).collect(),
            Doc::Array(a) => a
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(self, Doc::Object(_) | Doc::Array(_))
    }

    /// One-line text for a cell: containers are summarised, scalars shown plainly.
    pub fn summary(&self) -> String {
        match self {
            Doc::Object(f) => format!(
                "{{ {} field{} }}",
                f.len(),
                if f.len() == 1 { "" } else { "s" }
            ),
            Doc::Array(a) => format!(
                "[ {} item{} ]",
                a.len(),
                if a.len() == 1 { "" } else { "s" }
            ),
            Doc::Null => "null".into(),
            Doc::Bool(b) => b.to_string(),
            Doc::Int32(n) => n.to_string(),
            Doc::Int64(n) => n.to_string(),
            Doc::Double(f) => fmt_double(*f),
            Doc::Decimal(s) | Doc::String(s) | Doc::ObjectId(s) | Doc::Date(s) => s.clone(),
            Doc::Binary(s) | Doc::Regex(s) => s.clone(),
            Doc::Other { text, .. } => text.clone(),
        }
    }

    /// Pretty-printed shell syntax, two-space indent.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out
    }

    fn write_pretty(&self, out: &mut String, depth: usize) {
        let pad = |d: usize| "  ".repeat(d);
        match self {
            Doc::Object(f) if f.is_empty() => out.push_str("{}"),
            Doc::Array(a) if a.is_empty() => out.push_str("[]"),
            Doc::Object(f) => {
                out.push_str("{\n");
                for (i, (k, v)) in f.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    out.push_str(&quote(k));
                    out.push_str(": ");
                    v.write_pretty(out, depth + 1);
                    if i + 1 < f.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(depth));
                out.push('}');
            }
            Doc::Array(a) => {
                out.push_str("[\n");
                for (i, v) in a.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    v.write_pretty(out, depth + 1);
                    if i + 1 < a.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(depth));
                out.push(']');
            }
            Doc::String(s) => out.push_str(&quote(s)),
            Doc::ObjectId(s) => out.push_str(&format!("ObjectId(\"{s}\")")),
            Doc::Date(s) => out.push_str(&format!("ISODate(\"{s}\")")),
            Doc::Decimal(s) => out.push_str(&format!("NumberDecimal(\"{s}\")")),
            Doc::Int64(n) => out.push_str(&format!("NumberLong({n})")),
            other => out.push_str(&other.summary()),
        }
    }
}

fn fmt_double(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Doc {
        Doc::Object(vec![
            (
                "_id".into(),
                Doc::ObjectId("65a1b2c3d4e5f60718293a4b".into()),
            ),
            ("name".into(), Doc::String("Ann".into())),
            ("age".into(), Doc::Int32(31)),
            (
                "address".into(),
                Doc::Object(vec![("city".into(), Doc::String("BKK".into()))]),
            ),
            (
                "tags".into(),
                Doc::Array(vec![Doc::String("a".into()), Doc::String("b".into())]),
            ),
            ("empty".into(), Doc::Array(vec![])),
        ])
    }

    #[test]
    fn summaries_and_types() {
        let d = sample();
        assert_eq!(d.summary(), "{ 6 fields }");
        assert_eq!(d.get("address").unwrap().summary(), "{ 1 field }");
        assert_eq!(d.get("tags").unwrap().summary(), "[ 2 items ]");
        assert_eq!(d.get("age").unwrap().type_name(), "Int32");
        assert_eq!(d.get("_id").unwrap().kind(), DocKind::Id);
        assert_eq!(d.get("tags").unwrap().children()[1].0, "1");
        assert_eq!(Doc::Double(2.0).summary(), "2.0");
    }

    #[test]
    fn pretty() {
        assert_eq!(
            sample().to_pretty(),
            "{\n  \"_id\": ObjectId(\"65a1b2c3d4e5f60718293a4b\"),\n  \"name\": \"Ann\",\n  \"age\": 31,\n  \"address\": {\n    \"city\": \"BKK\"\n  },\n  \"tags\": [\n    \"a\",\n    \"b\"\n  ],\n  \"empty\": []\n}"
        );
    }
}
