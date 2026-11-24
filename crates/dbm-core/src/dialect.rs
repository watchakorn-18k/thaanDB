//! SQL text generation per database family. The GUI never builds SQL strings itself.

use crate::schema::ObjectRef;
use crate::splitter::SplitOptions;
use crate::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaceholderStyle {
    /// `?`
    Question,
    /// `$1`, `$2`, …
    Dollar,
    /// `@P1`, `@P2`, … (SQL Server)
    AtP,
}

impl PlaceholderStyle {
    /// Placeholder for the `n`-th (1-based) bound value.
    pub fn render(self, n: usize) -> String {
        match self {
            PlaceholderStyle::Question => "?".into(),
            PlaceholderStyle::Dollar => format!("${n}"),
            PlaceholderStyle::AtP => format!("@P{n}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PagingStyle {
    /// `LIMIT n OFFSET m`
    LimitOffset,
    /// `OFFSET m ROWS FETCH NEXT n ROWS ONLY`
    OffsetFetch,
}

/// Describes one SQL family. Built-in families are available as constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dialect {
    pub name: &'static str,
    pub quote_open: char,
    pub quote_close: char,
    pub placeholders: PlaceholderStyle,
    pub paging: PagingStyle,
    pub split: SplitOptions,
    pub supports_returning: bool,
}

impl Dialect {
    pub const SQLITE: Dialect = Dialect {
        name: "sqlite",
        quote_open: '"',
        quote_close: '"',
        placeholders: PlaceholderStyle::Question,
        paging: PagingStyle::LimitOffset,
        split: SplitOptions {
            dollar_quotes: false,
            backslash_escapes: false,
            hash_comments: false,
            delimiter_command: false,
            bracket_identifiers: true,
        },
        supports_returning: true,
    };
    pub const POSTGRES: Dialect = Dialect {
        name: "postgres",
        quote_open: '"',
        quote_close: '"',
        placeholders: PlaceholderStyle::Dollar,
        paging: PagingStyle::LimitOffset,
        split: SplitOptions {
            dollar_quotes: true,
            backslash_escapes: false,
            hash_comments: false,
            delimiter_command: false,
            bracket_identifiers: false,
        },
        supports_returning: true,
    };
    pub const MYSQL: Dialect = Dialect {
        name: "mysql",
        quote_open: '`',
        quote_close: '`',
        placeholders: PlaceholderStyle::Question,
        paging: PagingStyle::LimitOffset,
        split: SplitOptions {
            dollar_quotes: false,
            backslash_escapes: true,
            hash_comments: true,
            delimiter_command: true,
            bracket_identifiers: false,
        },
        supports_returning: false,
    };
    pub const MSSQL: Dialect = Dialect {
        name: "mssql",
        quote_open: '[',
        quote_close: ']',
        placeholders: PlaceholderStyle::AtP,
        paging: PagingStyle::OffsetFetch,
        split: SplitOptions {
            dollar_quotes: false,
            backslash_escapes: false,
            hash_comments: false,
            delimiter_command: false,
            bracket_identifiers: true,
        },
        supports_returning: false,
    };

    pub fn quote_ident(&self, ident: &str) -> String {
        let close = self.quote_close.to_string();
        let escaped = ident.replace(&close, &format!("{close}{close}"));
        format!("{}{}{}", self.quote_open, escaped, self.quote_close)
    }

    pub fn quote_object(&self, o: &ObjectRef) -> String {
        match &o.schema {
            Some(s) => format!("{}.{}", self.quote_ident(s), self.quote_ident(&o.name)),
            None => self.quote_ident(&o.name),
        }
    }

    pub fn paging_clause(&self, limit: u64, offset: u64) -> String {
        match self.paging {
            PagingStyle::LimitOffset => format!("LIMIT {limit} OFFSET {offset}"),
            PagingStyle::OffsetFetch => {
                format!("OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY")
            }
        }
    }

    /// Renders a value as a SQL literal. Used only for *exported* SQL text and previews,
    /// never for executing user data (executions always bind parameters).
    pub fn literal(&self, v: &Value) -> String {
        match v {
            Value::Null => "NULL".into(),
            Value::Bool(b) => {
                if self.name == "postgres" {
                    if *b { "TRUE" } else { "FALSE" }.into()
                } else {
                    if *b { "1" } else { "0" }.into()
                }
            }
            Value::Int(i) => i.to_string(),
            Value::Float(f) if f.is_finite() => f.to_string(),
            Value::Float(_) => "NULL".into(),
            Value::Text(s) => {
                let mut s = s.replace('\'', "''");
                if self.split.backslash_escapes {
                    s = s.replace('\\', "\\\\");
                }
                format!("'{s}'")
            }
            Value::Bytes(b) => match self.name {
                "postgres" => format!("'\\x{}'", crate::value::hex(b)),
                "mssql" => format!("0x{}", crate::value::hex(b)),
                _ => format!("X'{}'", crate::value::hex(b)),
            },
        }
    }

    pub fn for_driver(driver: &str) -> Dialect {
        match driver {
            "postgres" | "cockroach" | "redshift" => Dialect::POSTGRES,
            "mysql" | "mariadb" | "tidb" => Dialect::MYSQL,
            "mssql" => Dialect::MSSQL,
            _ => Dialect::SQLITE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(Dialect::POSTGRES.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(Dialect::MYSQL.quote_ident("a`b"), "`a``b`");
        assert_eq!(Dialect::MSSQL.quote_ident("a]b"), "[a]]b]");
        let o = ObjectRef {
            schema: Some("public".into()),
            name: "users".into(),
        };
        assert_eq!(Dialect::POSTGRES.quote_object(&o), "\"public\".\"users\"");
    }

    #[test]
    fn literals() {
        assert_eq!(
            Dialect::SQLITE.literal(&Value::Text("it's".into())),
            "'it''s'"
        );
        assert_eq!(
            Dialect::MYSQL.literal(&Value::Text("a\\b".into())),
            "'a\\\\b'"
        );
        assert_eq!(
            Dialect::SQLITE.literal(&Value::Bytes(vec![1, 255])),
            "X'01ff'"
        );
        assert_eq!(Dialect::POSTGRES.literal(&Value::Bool(true)), "TRUE");
    }
}
