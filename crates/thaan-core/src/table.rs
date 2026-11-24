//! Table data editor: paged fetch with filters and sort, and staged changes.

use crate::dialect::{Dialect, PagingStyle};
use crate::schema::ObjectRef;
use crate::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    NotLike,
    IsNull,
    IsNotNull,
    In,
}

impl FilterOp {
    pub const ALL: [FilterOp; 11] = [
        FilterOp::Eq,
        FilterOp::Ne,
        FilterOp::Lt,
        FilterOp::Le,
        FilterOp::Gt,
        FilterOp::Ge,
        FilterOp::Like,
        FilterOp::NotLike,
        FilterOp::IsNull,
        FilterOp::IsNotNull,
        FilterOp::In,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterOp::Eq => "=",
            FilterOp::Ne => "!=",
            FilterOp::Lt => "<",
            FilterOp::Le => "<=",
            FilterOp::Gt => ">",
            FilterOp::Ge => ">=",
            FilterOp::Like => "like",
            FilterOp::NotLike => "not like",
            FilterOp::IsNull => "is null",
            FilterOp::IsNotNull => "is not null",
            FilterOp::In => "in",
        }
    }

    /// The inverse of [`FilterOp::label`].
    pub fn from_label(label: &str) -> Option<FilterOp> {
        FilterOp::ALL.into_iter().find(|op| op.label() == label)
    }

    /// `false` for `is null` / `is not null`, which take no value.
    pub fn needs_value(self) -> bool {
        !matches!(self, FilterOp::IsNull | FilterOp::IsNotNull)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub column: String,
    pub op: FilterOp,
    /// Ignored for `isNull` / `isNotNull`; a list for `in`.
    #[serde(default)]
    pub values: Vec<Value>,
}

impl Filter {
    /// Builds a filter from what a user typed in a filter row.
    ///
    /// `data_type` is the column's declared type, if known. Numeric and boolean columns get
    /// their value parsed so it binds with the matching type; text that does not parse stays
    /// text. `like` / `not like` always bind text. `in` takes a comma-separated list.
    ///
    /// Returns `None` when the operator needs a value and none was typed, so an empty box
    /// means "no filter" and never `= ''`.
    pub fn from_text(
        column: &str,
        op: FilterOp,
        text: &str,
        data_type: Option<&str>,
    ) -> Option<Filter> {
        let values = if !op.needs_value() {
            Vec::new()
        } else if op == FilterOp::In {
            let vs: Vec<Value> = text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| typed_value(s, data_type))
                .collect();
            if vs.is_empty() {
                return None;
            }
            vs
        } else {
            let t = text.trim();
            if t.is_empty() {
                return None;
            }
            if matches!(op, FilterOp::Like | FilterOp::NotLike) {
                vec![Value::Text(t.to_string())]
            } else {
                vec![typed_value(t, data_type)]
            }
        };
        Some(Filter {
            column: column.to_string(),
            op,
            values,
        })
    }
}

/// Parses `s` by the column's declared type, using SQLite-style affinity rules that also
/// fit the common type names of other databases.
fn typed_value(s: &str, data_type: Option<&str>) -> Value {
    let t = data_type.unwrap_or("").to_ascii_uppercase();
    let parsed = if t.contains("INT") || t == "SERIAL" || t == "BIGSERIAL" {
        s.parse().ok().map(Value::Int)
    } else if ["REAL", "FLOA", "DOUB", "NUMERIC", "DECIMAL"]
        .iter()
        .any(|k| t.contains(k))
    {
        s.parse::<i64>()
            .map(Value::Int)
            .or_else(|_| s.parse().map(Value::Float))
            .ok()
    } else if t.contains("BOOL") {
        match s.to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" => Some(Value::Bool(true)),
            "false" | "f" | "0" | "no" => Some(Value::Bool(false)),
            _ => None,
        }
    } else {
        None
    };
    parsed.unwrap_or_else(|| Value::Text(s.to_string()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sort {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    pub table: ObjectRef,
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Raw `WHERE` text typed by the user (combined with `filters` using AND).
    #[serde(default)]
    pub raw_where: Option<String>,
    #[serde(default)]
    pub sort: Vec<Sort>,
    /// 0-based page index.
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

fn default_page_size() -> u64 {
    100
}

/// A SQL statement plus its bound values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundSql {
    pub sql: String,
    pub params: Vec<Value>,
}

impl BoundSql {
    /// Inlines parameters as literals, for display only.
    pub fn preview(&self, d: &Dialect) -> String {
        let mut out = self.sql.clone();
        // Replace from the highest index down so `$10` is not hit by `$1`.
        for (i, v) in self.params.iter().enumerate().rev() {
            let ph = d.placeholders.render(i + 1);
            if ph == "?" {
                continue;
            }
            out = out.replace(&ph, &d.literal(v));
        }
        if d.placeholders.render(1) == "?" {
            let mut s = String::new();
            let mut it = self.params.iter();
            for ch in out.chars() {
                match (ch, ch == '?') {
                    (_, true) => s.push_str(&it.next().map(|v| d.literal(v)).unwrap_or("?".into())),
                    (c, false) => s.push(c),
                }
            }
            out = s;
        }
        out
    }
}

struct Builder<'a> {
    d: &'a Dialect,
    params: Vec<Value>,
}

impl Builder<'_> {
    fn bind(&mut self, v: Value) -> String {
        self.params.push(v);
        self.d.placeholders.render(self.params.len())
    }

    fn where_clause(&mut self, filters: &[Filter], raw: Option<&str>) -> String {
        let mut parts = Vec::new();
        for f in filters {
            let col = self.d.quote_ident(&f.column);
            let first = f.values.first().cloned().unwrap_or_default();
            let p = match f.op {
                FilterOp::IsNull => format!("{col} IS NULL"),
                FilterOp::IsNotNull => format!("{col} IS NOT NULL"),
                FilterOp::In => {
                    if f.values.is_empty() {
                        "1 = 0".to_string()
                    } else {
                        let phs: Vec<_> = f.values.iter().map(|v| self.bind(v.clone())).collect();
                        format!("{col} IN ({})", phs.join(", "))
                    }
                }
                op => {
                    let sym = match op {
                        FilterOp::Eq => "=",
                        FilterOp::Ne => "<>",
                        FilterOp::Lt => "<",
                        FilterOp::Le => "<=",
                        FilterOp::Gt => ">",
                        FilterOp::Ge => ">=",
                        FilterOp::Like => "LIKE",
                        _ => "NOT LIKE",
                    };
                    format!("{col} {sym} {}", self.bind(first))
                }
            };
            parts.push(p);
        }
        if let Some(r) = raw.map(str::trim).filter(|r| !r.is_empty()) {
            parts.push(format!("({r})"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", parts.join(" AND "))
        }
    }
}

/// Builds the page query and the matching `COUNT(*)` query.
pub fn page_queries(d: &Dialect, req: &PageRequest) -> (BoundSql, BoundSql) {
    let table = d.quote_object(&req.table);
    let mut b = Builder {
        d,
        params: Vec::new(),
    };
    let wh = b.where_clause(&req.filters, req.raw_where.as_deref());
    let where_params = b.params.clone();
    let mut order = req
        .sort
        .iter()
        .map(|s| {
            format!(
                "{}{}",
                d.quote_ident(&s.column),
                if s.descending { " DESC" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if order.is_empty() && d.paging == PagingStyle::OffsetFetch {
        order = "(SELECT NULL)".into();
    }
    let order = if order.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {order}")
    };
    let size = req.page_size.max(1);
    let page = BoundSql {
        sql: format!(
            "SELECT * FROM {table}{wh}{order} {}",
            d.paging_clause(size, req.page * size)
        ),
        params: b.params,
    };
    let count = BoundSql {
        sql: format!("SELECT COUNT(*) FROM {table}{wh}"),
        params: where_params,
    };
    (page, count)
}

/// Every row matching the request's filters and sort, without paging (for export).
pub fn select_all(d: &Dialect, req: &PageRequest) -> BoundSql {
    let (page, _) = page_queries(d, req);
    let cut = page.sql.rfind(&format!(
        " {}",
        d.paging_clause(req.page_size.max(1), req.page * req.page_size.max(1))
    ));
    let sql = match cut {
        Some(i) => page.sql[..i].to_string(),
        None => page.sql,
    };
    BoundSql {
        sql,
        params: page.params,
    }
}

/// One staged edit from the data grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StagedChange {
    Insert {
        values: Vec<(String, Value)>,
    },
    /// `key` identifies the row (primary key, or all original values when there is no key).
    Update {
        key: Vec<(String, Value)>,
        set: Vec<(String, Value)>,
    },
    Delete {
        key: Vec<(String, Value)>,
    },
}

/// SQL for a list of staged changes, in order. Apply them in one transaction.
pub fn change_statements(
    d: &Dialect,
    table: &ObjectRef,
    changes: &[StagedChange],
) -> Vec<BoundSql> {
    let t = d.quote_object(table);
    changes
        .iter()
        .map(|c| {
            let mut b = Builder {
                d,
                params: Vec::new(),
            };
            let sql = match c {
                StagedChange::Insert { values } if values.is_empty() => {
                    format!("INSERT INTO {t} DEFAULT VALUES")
                }
                StagedChange::Insert { values } => {
                    let cols: Vec<_> = values.iter().map(|(c, _)| d.quote_ident(c)).collect();
                    let phs: Vec<_> = values.iter().map(|(_, v)| b.bind(v.clone())).collect();
                    format!(
                        "INSERT INTO {t} ({}) VALUES ({})",
                        cols.join(", "),
                        phs.join(", ")
                    )
                }
                StagedChange::Update { key, set } => {
                    let sets: Vec<_> = set
                        .iter()
                        .map(|(c, v)| format!("{} = {}", d.quote_ident(c), b.bind(v.clone())))
                        .collect();
                    let wh = key_where(&mut b, key);
                    format!("UPDATE {t} SET {} WHERE {wh}", sets.join(", "))
                }
                StagedChange::Delete { key } => {
                    let wh = key_where(&mut b, key);
                    format!("DELETE FROM {t} WHERE {wh}")
                }
            };
            BoundSql {
                sql,
                params: b.params,
            }
        })
        .collect()
}

fn key_where(b: &mut Builder<'_>, key: &[(String, Value)]) -> String {
    if key.is_empty() {
        // Never generate an unbounded UPDATE/DELETE.
        return "1 = 0".into();
    }
    key.iter()
        .map(|(c, v)| {
            let col = b.d.quote_ident(c);
            if v.is_null() {
                format!("{col} IS NULL")
            } else {
                format!("{col} = {}", b.bind(v.clone()))
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> PageRequest {
        PageRequest {
            table: ObjectRef::new("users"),
            filters: vec![
                Filter {
                    column: "age".into(),
                    op: FilterOp::Ge,
                    values: vec![Value::Int(18)],
                },
                Filter {
                    column: "email".into(),
                    op: FilterOp::IsNotNull,
                    values: vec![],
                },
            ],
            raw_where: Some("name like 'a%'".into()),
            sort: vec![Sort {
                column: "id".into(),
                descending: true,
            }],
            page: 2,
            page_size: 50,
        }
    }

    #[test]
    fn page_query_postgres() {
        let (p, c) = page_queries(&Dialect::POSTGRES, &req());
        assert_eq!(
            p.sql,
            "SELECT * FROM \"users\" WHERE \"age\" >= $1 AND \"email\" IS NOT NULL AND (name like 'a%') ORDER BY \"id\" DESC LIMIT 50 OFFSET 100"
        );
        assert_eq!(p.params, [Value::Int(18)]);
        assert!(c.sql.starts_with("SELECT COUNT(*) FROM \"users\" WHERE"));
    }

    #[test]
    fn page_query_mssql_needs_order() {
        let r = PageRequest {
            sort: vec![],
            filters: vec![],
            raw_where: None,
            ..req()
        };
        let (p, _) = page_queries(&Dialect::MSSQL, &r);
        assert_eq!(
            p.sql,
            "SELECT * FROM [users] ORDER BY (SELECT NULL) OFFSET 100 ROWS FETCH NEXT 50 ROWS ONLY"
        );
    }

    #[test]
    fn filter_from_text() {
        let f = |op, text, ty| Filter::from_text("c", op, text, ty).map(|f| f.values);
        // Empty value means no filter, never `= ''`.
        assert_eq!(f(FilterOp::Eq, "  ", Some("TEXT")), None);
        assert_eq!(f(FilterOp::In, " , ", Some("INT")), None);
        // Null checks take no value.
        assert_eq!(f(FilterOp::IsNull, "", None), Some(vec![]));
        // Values are typed by the column's declared type.
        assert_eq!(f(FilterOp::Eq, "42", Some("integer")), Some(vec![Value::Int(42)]));
        assert_eq!(f(FilterOp::Gt, "1.5", Some("REAL")), Some(vec![Value::Float(1.5)]));
        assert_eq!(f(FilterOp::Eq, "t", Some("boolean")), Some(vec![Value::Bool(true)]));
        // A text column keeps leading zeros; unparsable numbers stay text.
        assert_eq!(f(FilterOp::Eq, "007", Some("TEXT")), Some(vec![Value::Text("007".into())]));
        assert_eq!(f(FilterOp::Eq, "abc", Some("INT")), Some(vec![Value::Text("abc".into())]));
        // `like` always binds text; `in` splits on commas.
        assert_eq!(f(FilterOp::Like, "4%", Some("INT")), Some(vec![Value::Text("4%".into())]));
        assert_eq!(
            f(FilterOp::In, "1, 2,,3", Some("INT")),
            Some(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        assert_eq!(FilterOp::from_label("not like"), Some(FilterOp::NotLike));
    }

    #[test]
    fn staged_changes() {
        let t = ObjectRef::new("users");
        let s = change_statements(
            &Dialect::SQLITE,
            &t,
            &[
                StagedChange::Insert {
                    values: vec![("name".into(), "ann".into())],
                },
                StagedChange::Update {
                    key: vec![("id".into(), Value::Int(1))],
                    set: vec![("name".into(), "bob".into())],
                },
                StagedChange::Delete {
                    key: vec![("id".into(), Value::Int(2)), ("x".into(), Value::Null)],
                },
                StagedChange::Delete { key: vec![] },
            ],
        );
        assert_eq!(s[0].sql, "INSERT INTO \"users\" (\"name\") VALUES (?)");
        assert_eq!(
            s[1].sql,
            "UPDATE \"users\" SET \"name\" = ? WHERE \"id\" = ?"
        );
        assert_eq!(s[1].params, [Value::from("bob"), Value::Int(1)]);
        assert_eq!(
            s[2].sql,
            "DELETE FROM \"users\" WHERE \"id\" = ? AND \"x\" IS NULL"
        );
        assert_eq!(s[3].sql, "DELETE FROM \"users\" WHERE 1 = 0");
        assert_eq!(
            s[1].preview(&Dialect::SQLITE),
            "UPDATE \"users\" SET \"name\" = 'bob' WHERE \"id\" = 1"
        );
        let pg = change_statements(
            &Dialect::POSTGRES,
            &t,
            &[StagedChange::Update {
                key: vec![("id".into(), Value::Int(1))],
                set: vec![("name".into(), "o'k".into())],
            }],
        );
        assert_eq!(
            pg[0].preview(&Dialect::POSTGRES),
            "UPDATE \"users\" SET \"name\" = 'o''k' WHERE \"id\" = 1"
        );
    }
}
