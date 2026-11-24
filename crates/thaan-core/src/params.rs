//! Query parameters: `:name`, `$1` and `?` placeholders in user SQL.
//!
//! The editor asks the user for a value per placeholder, then [`bind`] rewrites named
//! placeholders into the driver's positional style and orders the values.

use crate::dialect::PlaceholderStyle;
use crate::{Error, Result, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "key", rename_all = "lowercase")]
pub enum Placeholder {
    /// `:name`
    Named(String),
    /// `$1` (1-based)
    Numbered(u32),
    /// `?` (0-based occurrence index)
    Positional(u32),
}

impl Placeholder {
    /// Key used in the parameter map the user fills in.
    pub fn key(&self) -> String {
        match self {
            Placeholder::Named(n) => n.clone(),
            Placeholder::Numbered(i) => format!("${i}"),
            Placeholder::Positional(i) => format!("?{}", i + 1),
        }
    }
}

struct Hit {
    start: usize,
    end: usize,
    ph: Placeholder,
}

fn scan(sql: &str) -> Vec<Hit> {
    let b = sql.as_bytes();
    let mut hits = Vec::new();
    let mut i = 0;
    let mut positional = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                let q = b[i];
                i += 1;
                while i < b.len() {
                    if b[i] == q {
                        if i + 1 < b.len() && b[i + 1] == q {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                i = sql[i..].find('\n').map(|p| i + p).unwrap_or(b.len());
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..]
                    .find("*/")
                    .map(|p| i + 4 + p)
                    .unwrap_or(b.len());
            }
            b':' => {
                let prev_colon = i > 0 && b[i - 1] == b':';
                let next = b.get(i + 1).copied().unwrap_or(0);
                if !prev_colon && next != b':' && (next.is_ascii_alphabetic() || next == b'_') {
                    let mut j = i + 1;
                    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                        j += 1;
                    }
                    hits.push(Hit {
                        start: i,
                        end: j,
                        ph: Placeholder::Named(sql[i + 1..j].into()),
                    });
                    i = j;
                } else {
                    i += 1;
                }
            }
            b'$' if b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let n = sql[i + 1..j].parse().unwrap_or(0);
                hits.push(Hit {
                    start: i,
                    end: j,
                    ph: Placeholder::Numbered(n),
                });
                i = j;
            }
            b'?' => {
                hits.push(Hit {
                    start: i,
                    end: i + 1,
                    ph: Placeholder::Positional(positional),
                });
                positional += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    hits
}

/// Distinct placeholders in order of first appearance.
pub fn find(sql: &str) -> Vec<Placeholder> {
    let mut out: Vec<Placeholder> = Vec::new();
    for h in scan(sql) {
        if !out.contains(&h.ph) {
            out.push(h.ph);
        }
    }
    out
}

/// Rewrites `sql` for the driver's placeholder style and returns the values in bind order.
/// `values` is keyed by [`Placeholder::key`].
pub fn bind(
    sql: &str,
    values: &BTreeMap<String, Value>,
    style: PlaceholderStyle,
) -> Result<(String, Vec<Value>)> {
    let hits = scan(sql);
    if hits.is_empty() {
        return Ok((sql.to_string(), Vec::new()));
    }
    let get = |ph: &Placeholder| {
        values.get(&ph.key()).cloned().ok_or_else(|| {
            Error::InvalidParams(format!("missing value for parameter {}", ph.key()))
        })
    };
    let mut out = String::with_capacity(sql.len());
    let mut bound = Vec::new();
    let mut last = 0;
    for h in &hits {
        out.push_str(&sql[last..h.start]);
        bound.push(get(&h.ph)?);
        out.push_str(&style.render(bound.len()));
        last = h.end;
    }
    out.push_str(&sql[last..]);
    Ok((out, bound))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_placeholders() {
        let p =
            find("select * from t where a = :a and b = :b and c = :a and d::int = 1 and e = ':x'");
        assert_eq!(
            p,
            [
                Placeholder::Named("a".into()),
                Placeholder::Named("b".into())
            ]
        );
        assert_eq!(
            find("select $1, $2, $1"),
            [Placeholder::Numbered(1), Placeholder::Numbered(2)]
        );
        assert_eq!(find("select ?, ?").len(), 2);
    }

    #[test]
    fn binds_named_to_positional() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), Value::Int(1));
        v.insert("b".into(), Value::Text("x".into()));
        let (sql, vals) = bind("select :a, :b, :a", &v, PlaceholderStyle::Dollar).unwrap();
        assert_eq!(sql, "select $1, $2, $3");
        assert_eq!(
            vals,
            [Value::Int(1), Value::Text("x".into()), Value::Int(1)]
        );
        let (sql, _) = bind("select :a", &v, PlaceholderStyle::Question).unwrap();
        assert_eq!(sql, "select ?");
        assert!(bind("select :zzz", &v, PlaceholderStyle::Question).is_err());
    }
}
