//! Result export / copy formats.

use crate::dialect::Dialect;
use crate::{ResultSet, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    JsonLines,
    SqlInsert,
    Markdown,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [
        ExportFormat::Csv,
        ExportFormat::Tsv,
        ExportFormat::Json,
        ExportFormat::JsonLines,
        ExportFormat::SqlInsert,
        ExportFormat::Markdown,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::JsonLines => "jsonl",
            ExportFormat::SqlInsert => "sql",
            ExportFormat::Markdown => "md",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Tsv => "TSV",
            ExportFormat::Json => "JSON",
            ExportFormat::JsonLines => "JSON Lines",
            ExportFormat::SqlInsert => "SQL INSERT",
            ExportFormat::Markdown => "Markdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    #[serde(default = "yes")]
    pub include_header: bool,
    /// Table name used by `sqlInsert`.
    #[serde(default)]
    pub table_name: Option<String>,
    /// Text written for NULL in CSV/TSV/Markdown.
    #[serde(default)]
    pub null_text: String,
}

fn yes() -> bool {
    true
}

impl ExportOptions {
    pub fn new(format: ExportFormat) -> Self {
        Self {
            format,
            include_header: true,
            table_name: None,
            null_text: String::new(),
        }
    }
}

pub fn export(rs: &ResultSet, opts: &ExportOptions, dialect: &Dialect) -> String {
    let mut out = String::new();
    match opts.format {
        ExportFormat::Csv | ExportFormat::Tsv => {
            let sep = if opts.format == ExportFormat::Csv {
                ','
            } else {
                '\t'
            };
            let cell = |s: &str| -> String {
                let needs =
                    s.contains(sep) || s.contains('"') || s.contains('\n') || s.contains('\r');
                if needs {
                    format!("\"{}\"", s.replace('"', "\"\""))
                } else {
                    s.to_string()
                }
            };
            if opts.include_header {
                let h: Vec<_> = rs.columns.iter().map(|c| cell(&c.name)).collect();
                out.push_str(&h.join(&sep.to_string()));
                out.push_str("\r\n");
            }
            for row in &rs.rows {
                let r: Vec<_> = row
                    .iter()
                    .map(|v| {
                        if v.is_null() {
                            cell(&opts.null_text)
                        } else {
                            cell(&v.to_string())
                        }
                    })
                    .collect();
                out.push_str(&r.join(&sep.to_string()));
                out.push_str("\r\n");
            }
        }
        ExportFormat::Json => {
            let rows = rs.to_json_rows();
            out = serde_json::to_string_pretty(&rows).unwrap_or_default();
            out.push('\n');
        }
        ExportFormat::JsonLines => {
            for r in rs.to_json_rows() {
                out.push_str(&r.to_string());
                out.push('\n');
            }
        }
        ExportFormat::SqlInsert => {
            let table = dialect.quote_ident(opts.table_name.as_deref().unwrap_or("exported"));
            let cols: Vec<_> = rs
                .columns
                .iter()
                .map(|c| dialect.quote_ident(&c.name))
                .collect();
            for row in &rs.rows {
                let vals: Vec<_> = row.iter().map(|v| dialect.literal(v)).collect();
                out.push_str(&format!(
                    "INSERT INTO {table} ({}) VALUES ({});\n",
                    cols.join(", "),
                    vals.join(", ")
                ));
            }
        }
        ExportFormat::Markdown => {
            let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
            let h: Vec<_> = rs.columns.iter().map(|c| esc(&c.name)).collect();
            out.push_str(&format!("| {} |\n", h.join(" | ")));
            out.push_str(&format!("|{}\n", " --- |".repeat(rs.columns.len())));
            for row in &rs.rows {
                let r: Vec<_> = row
                    .iter()
                    .map(|v| {
                        if let Value::Null = v {
                            esc(&opts.null_text)
                        } else {
                            esc(&v.to_string())
                        }
                    })
                    .collect();
                out.push_str(&format!("| {} |\n", r.join(" | ")));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Column;

    fn rs() -> ResultSet {
        ResultSet {
            columns: vec![Column::new("id"), Column::new("name")],
            rows: vec![
                vec![Value::Int(1), Value::from("a,b")],
                vec![Value::Int(2), Value::Null],
                vec![Value::Int(3), Value::from("say \"hi\"")],
            ],
            ..Default::default()
        }
    }

    #[test]
    fn csv() {
        let s = export(
            &rs(),
            &ExportOptions::new(ExportFormat::Csv),
            &Dialect::SQLITE,
        );
        assert_eq!(s, "id,name\r\n1,\"a,b\"\r\n2,\r\n3,\"say \"\"hi\"\"\"\r\n");
    }

    #[test]
    fn json_and_lines() {
        let s = export(
            &rs(),
            &ExportOptions::new(ExportFormat::JsonLines),
            &Dialect::SQLITE,
        );
        assert_eq!(s.lines().next().unwrap(), r#"{"id":1,"name":"a,b"}"#);
        let j: serde_json::Value = serde_json::from_str(&export(
            &rs(),
            &ExportOptions::new(ExportFormat::Json),
            &Dialect::SQLITE,
        ))
        .unwrap();
        assert_eq!(j[1]["name"], serde_json::Value::Null);
    }

    #[test]
    fn sql_insert() {
        let mut o = ExportOptions::new(ExportFormat::SqlInsert);
        o.table_name = Some("t".into());
        let s = export(&rs(), &o, &Dialect::POSTGRES);
        assert_eq!(
            s.lines().nth(1).unwrap(),
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES (2, NULL);"
        );
    }

    #[test]
    fn markdown() {
        let s = export(
            &rs(),
            &ExportOptions::new(ExportFormat::Markdown),
            &Dialect::SQLITE,
        );
        assert!(s.starts_with("| id | name |\n| --- | --- |\n| 1 | a,b |\n"));
    }
}
