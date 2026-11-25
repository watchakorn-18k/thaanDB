//! Data transfer: copy tables (or collections) from one open session to another, across
//! servers and database engines. Structure is rebuilt from the source's columns with
//! types mapped to the target's dialect; rows are read one page at a time and written in
//! batches, so memory stays at one page whatever the table size. Progress is reported
//! after every batch and the copy stops at the next batch once cancelled.

use crate::dialect::{Dialect, QueryLanguage};
use crate::engine::{Engine, QueryRequest, RunScope, SessionId};
use crate::object_ops::{self, ObjectOp};
use crate::schema::{ColumnInfo, ObjectKind, ObjectRef};
use crate::table::{PageRequest, StagedChange};
use crate::{Error, Result, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// What to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TransferContent {
    #[default]
    StructureAndData,
    /// Create the tables only.
    StructureOnly,
    /// Rows only, into tables that already exist on the target.
    DataOnly,
}

/// What to do with a target table that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ExistingTable {
    /// Keep its rows and add the copied ones.
    #[default]
    Append,
    /// Delete its rows first.
    Truncate,
    /// Drop it and create it again from the source.
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRequest {
    pub source: SessionId,
    pub target: SessionId,
    /// Source tables or collections; each is written under the same name on the target.
    pub tables: Vec<ObjectRef>,
    #[serde(default)]
    pub content: TransferContent,
    #[serde(default)]
    pub existing: ExistingTable,
    /// Rows read and written per round trip.
    #[serde(default = "default_batch")]
    pub batch_size: u64,
}

fn default_batch() -> u64 {
    500
}

/// Where a running transfer is, after each batch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    /// 0-based index into `TransferRequest::tables`.
    pub table_ix: usize,
    pub table: String,
    pub rows_done: u64,
    pub rows_total: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    pub tables: usize,
    pub rows: u64,
    /// `true` when it stopped early because it was cancelled.
    pub cancelled: bool,
}

/// Broad column families, the common ground between engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Int,
    Bool,
    Float,
    Decimal,
    Date,
    DateTime,
    Json,
    Bytes,
    Text,
}

fn family(data_type: &str) -> Family {
    let t = data_type.to_ascii_lowercase();
    let has = |s: &str| t.contains(s);
    if has("bool") || t == "bit" || t == "tinyint(1)" {
        Family::Bool
    } else if has("int") || t == "serial" || t == "bigserial" {
        Family::Int
    } else if has("numeric") || has("decimal") || has("money") {
        Family::Decimal
    } else if has("float") || has("double") || has("real") {
        Family::Float
    } else if has("timestamp") || has("datetime") {
        Family::DateTime
    } else if t == "date" {
        Family::Date
    } else if has("json") || t == "object" || t == "array" {
        Family::Json
    } else if has("blob") || has("bytea") || has("binary") {
        Family::Bytes
    } else {
        Family::Text
    }
}

/// The target column type for a source column, in the target's dialect.
fn target_type(d: &Dialect, c: &ColumnInfo) -> &'static str {
    let key = c.primary_key.is_some();
    match (d.name, family(&c.data_type)) {
        ("postgres", Family::Int) => "bigint",
        ("postgres", Family::Bool) => "boolean",
        ("postgres", Family::Float) => "double precision",
        ("postgres", Family::Decimal) => "numeric",
        ("postgres", Family::Date) => "date",
        ("postgres", Family::DateTime) => "timestamptz",
        ("postgres", Family::Json) => "jsonb",
        ("postgres", Family::Bytes) => "bytea",
        ("postgres", Family::Text) => "text",
        ("mysql", Family::Int) => "BIGINT",
        ("mysql", Family::Bool) => "BOOLEAN",
        ("mysql", Family::Float) => "DOUBLE",
        ("mysql", Family::Decimal) => "DECIMAL(38,10)",
        ("mysql", Family::Date) => "DATE",
        ("mysql", Family::DateTime) => "DATETIME(6)",
        ("mysql", Family::Json) => "JSON",
        ("mysql", Family::Bytes) => "LONGBLOB",
        // MySQL cannot index unbounded text, so a text key gets a length.
        ("mysql", Family::Text) if key => "VARCHAR(255)",
        ("mysql", Family::Text) => "LONGTEXT",
        ("mssql", Family::Int) => "BIGINT",
        ("mssql", Family::Bool) => "BIT",
        ("mssql", Family::Float) => "FLOAT",
        ("mssql", Family::Decimal) => "DECIMAL(38,10)",
        ("mssql", Family::Date) => "DATE",
        ("mssql", Family::DateTime) => "DATETIME2",
        ("mssql", Family::Bytes) => "VARBINARY(MAX)",
        ("mssql", Family::Text | Family::Json) if key => "NVARCHAR(450)",
        ("mssql", Family::Text | Family::Json) => "NVARCHAR(MAX)",
        (_, Family::Int | Family::Bool) => "INTEGER",
        (_, Family::Float) => "REAL",
        (_, Family::Decimal) => "NUMERIC",
        (_, Family::Bytes) => "BLOB",
        _ => "TEXT",
    }
}

/// CREATE TABLE for `target` with the source's columns, types mapped to `d`. Keys and
/// NOT NULL carry over; defaults, indexes and foreign keys do not (they are written in
/// each engine's own language).
pub fn create_table_sql(d: &Dialect, target: &ObjectRef, columns: &[ColumnInfo]) -> String {
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "  {} {}{}",
                d.quote_ident(&c.name),
                target_type(d, c),
                if c.nullable { "" } else { " NOT NULL" }
            )
        })
        .collect();
    let mut key: Vec<&ColumnInfo> = columns.iter().filter(|c| c.primary_key.is_some()).collect();
    key.sort_by_key(|c| c.primary_key);
    if !key.is_empty() {
        let names: Vec<String> = key.iter().map(|c| d.quote_ident(&c.name)).collect();
        lines.push(format!("  PRIMARY KEY ({})", names.join(", ")));
    }
    format!(
        "CREATE TABLE {} (\n{}\n)",
        d.quote_object(target),
        lines.join(",\n")
    )
}

impl Engine {
    /// Copies `req.tables` from the source session to the target session. `progress`
    /// runs after every batch; setting `cancel` stops before the next one. Writes are
    /// refused on a read-only target. A table that fails stops the transfer with its
    /// error (earlier tables stay copied).
    pub fn transfer(
        &self,
        req: &TransferRequest,
        progress: &dyn Fn(&TransferProgress),
        cancel: &AtomicBool,
    ) -> Result<TransferReport> {
        let target = self.session_info_of(req.target)?;
        if target.0 {
            return Err(Error::ReadOnly("data transfer".into()));
        }
        let td = target.1;
        if matches!(td.language, QueryLanguage::Redis | QueryLanguage::Mqtt) {
            // Keys and topics have no table shape to copy rows into.
            return Err(Error::Unsupported(format!(
                "copying tables into {}",
                td.name
            )));
        }
        let mut report = TransferReport::default();
        let existing: Vec<String> = self
            .list_objects(req.target)?
            .into_iter()
            .filter(|o| o.kind == ObjectKind::Table)
            .map(|o| o.object.name)
            .collect();
        for (ix, source_table) in req.tables.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                break;
            }
            let dest = ObjectRef {
                schema: None,
                name: source_table.name.clone(),
            };
            let exists = existing.iter().any(|n| n == &dest.name);
            let run = |sql: String| -> Result<()> {
                let r = self.run_query(&QueryRequest {
                    session_id: req.target,
                    sql,
                    scope: RunScope::All,
                    params: BTreeMap::new(),
                })?;
                match r.into_iter().find_map(|x| x.error) {
                    Some(e) => Err(Error::Database(e)),
                    None => Ok(()),
                }
            };
            // Structure.
            if req.content != TransferContent::DataOnly {
                let mut create = !exists;
                if exists && req.existing == ExistingTable::Replace {
                    for sql in
                        object_ops::statements(&td, ObjectKind::Table, &dest, &ObjectOp::Drop)?
                    {
                        run(sql)?;
                    }
                    create = true;
                }
                if create {
                    if td.language == QueryLanguage::MongoShell {
                        for sql in object_ops::statements(
                            &td,
                            ObjectKind::Table,
                            &dest,
                            &ObjectOp::Create,
                        )? {
                            run(sql)?;
                        }
                    } else {
                        let info = self.describe_table(req.source, source_table)?;
                        run(create_table_sql(&td, &dest, &info.columns))?;
                    }
                }
            } else if !exists && td.language != QueryLanguage::MongoShell {
                return Err(Error::InvalidParams(format!(
                    "{} does not exist on the target (Data only copies into existing tables)",
                    dest.name
                )));
            }
            if exists && req.existing == ExistingTable::Truncate {
                for sql in
                    object_ops::statements(&td, ObjectKind::Table, &dest, &ObjectOp::Truncate)?
                {
                    run(sql)?;
                }
            }
            report.tables += 1;
            if req.content == TransferContent::StructureOnly {
                continue;
            }
            // Rows, one page at a time.
            let batch = req.batch_size.clamp(1, 5000);
            let mut page = 0;
            let mut done = 0;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    report.cancelled = true;
                    return Ok(report);
                }
                let p = self.fetch_page(
                    req.source,
                    &PageRequest {
                        table: source_table.clone(),
                        filters: Vec::new(),
                        raw_where: None,
                        projection: None,
                        raw_sort: None,
                        sort: Vec::new(),
                        page,
                        page_size: batch,
                    },
                )?;
                let rows = p.result.rows.len() as u64;
                if rows == 0 {
                    break;
                }
                let names: Vec<String> = p.result.columns.iter().map(|c| c.name.clone()).collect();
                let changes: Vec<StagedChange> = p
                    .result
                    .rows
                    .into_iter()
                    .map(|row| StagedChange::Insert {
                        values: names
                            .iter()
                            .cloned()
                            .zip(row)
                            // Documents only carry the fields they have.
                            .filter(|(_, v)| {
                                !(v.is_null() && td.language == QueryLanguage::MongoShell)
                            })
                            .collect::<Vec<(String, Value)>>(),
                    })
                    .collect();
                self.apply_changes(req.target, &dest, &changes)?;
                done += rows;
                report.rows += rows;
                progress(&TransferProgress {
                    table_ix: ix,
                    table: dest.name.clone(),
                    rows_done: done,
                    rows_total: p.total_rows,
                });
                if rows < batch {
                    break;
                }
                page += 1;
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, t: &str, key: Option<u32>, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: t.into(),
            nullable,
            default: None,
            primary_key: key,
            comment: None,
            auto_increment: false,
        }
    }

    #[test]
    fn maps_types_across_engines() {
        let cols = [
            col("id", "integer", Some(1), false),
            col("email", "character varying(255)", None, true),
            col("paid", "boolean", None, true),
            col("at", "timestamp with time zone", None, true),
            col("meta", "jsonb", None, true),
        ];
        let t = ObjectRef {
            schema: None,
            name: "users".into(),
        };
        assert_eq!(
            create_table_sql(&Dialect::MYSQL, &t, &cols),
            "CREATE TABLE `users` (\n  `id` BIGINT NOT NULL,\n  `email` LONGTEXT,\n  `paid` BOOLEAN,\n  `at` DATETIME(6),\n  `meta` JSON,\n  PRIMARY KEY (`id`)\n)"
        );
        let sqlite = create_table_sql(&Dialect::SQLITE, &t, &cols);
        assert!(sqlite.contains("\"at\" TEXT"), "{sqlite}");
        assert!(sqlite.contains("PRIMARY KEY (\"id\")"));
        let mysql_text_key = [col("code", "text", Some(1), false)];
        assert!(create_table_sql(&Dialect::MYSQL, &t, &mysql_text_key).contains("VARCHAR(255)"));
    }
}
