//! Whole-object operations from the schema tree: rename, drop, truncate and duplicate a
//! table, view or collection. `statements` turns an operation into the exact text that
//! will run (SQL for the SQL dialects, shell commands for MongoDB), so the GUI can show
//! it before asking for confirmation, and `Engine::run_object_op` runs that same text.
//! Identifiers are always quoted by the dialect.

use crate::dialect::{Dialect, QueryLanguage};
use crate::schema::{ObjectKind, ObjectRef};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ObjectOp {
    /// New name, in the same schema.
    Rename {
        to: String,
    },
    Drop,
    /// Delete every row, keep the table.
    Truncate,
    /// A copy named `to` in the same schema, with or without its rows.
    Duplicate {
        to: String,
        #[serde(default = "yes")]
        with_data: bool,
    },
    /// Create an empty collection named by the object (MongoDB). SQL tables have
    /// columns to choose, so they start from `Dialect::create_table_template` instead.
    Create,
}

fn yes() -> bool {
    true
}

impl ObjectOp {
    /// Short verb for titles and history ("Rename", "Drop"…).
    pub fn verb(&self) -> &'static str {
        match self {
            ObjectOp::Rename { .. } => "Rename",
            ObjectOp::Drop => "Drop",
            ObjectOp::Truncate => "Truncate",
            ObjectOp::Duplicate { .. } => "Duplicate",
            ObjectOp::Create => "Create",
        }
    }

    /// Drop and truncate lose data and cannot be undone.
    pub fn destroys_data(&self) -> bool {
        matches!(self, ObjectOp::Drop | ObjectOp::Truncate)
    }
}

/// The statements that perform `op` on `object` (of `kind`), in order.
pub fn statements(
    dialect: &Dialect,
    kind: ObjectKind,
    object: &ObjectRef,
    op: &ObjectOp,
) -> Result<Vec<String>> {
    if let ObjectOp::Rename { to } | ObjectOp::Duplicate { to, .. } = op {
        let to = to.trim();
        if to.is_empty() {
            return Err(Error::InvalidParams("a new name is required".into()));
        }
        if to == object.name {
            return Err(Error::InvalidParams(
                "the new name is the same as the old one".into(),
            ));
        }
    }
    if *op == ObjectOp::Create && object.name.trim().is_empty() {
        return Err(Error::InvalidParams("a name is required".into()));
    }
    if dialect.language == QueryLanguage::MongoShell {
        return Ok(vec![mongo(object, op)]);
    }
    if dialect.language == QueryLanguage::Redis {
        return redis(object, op).map(|s| vec![s]);
    }
    if dialect.language == QueryLanguage::Mqtt {
        return Err(Error::Unsupported(format!(
            "{} on an MQTT topic (topics exist while messages arrive)",
            op.verb().to_lowercase()
        )));
    }
    if *op == ObjectOp::Create {
        return Err(Error::Unsupported(
            "create a table from its CREATE TABLE statement".into(),
        ));
    }
    let q = dialect.quote_object(object);
    let renamed = |to: &str| {
        dialect.quote_object(&ObjectRef {
            schema: object.schema.clone(),
            name: to.trim().to_string(),
        })
    };
    let noun = match kind {
        ObjectKind::Table => "TABLE",
        ObjectKind::View => "VIEW",
        ObjectKind::MaterializedView => "MATERIALIZED VIEW",
        other => {
            return Err(Error::Unsupported(format!(
                "{} on {}",
                op.verb(),
                other.label_plural()
            )))
        }
    };
    let only_tables = || {
        if kind == ObjectKind::Table {
            Ok(())
        } else {
            Err(Error::Unsupported(format!(
                "{} works on tables only",
                op.verb()
            )))
        }
    };
    let name = dialect.name;
    Ok(match op {
        ObjectOp::Rename { to } => match name {
            "mysql" => vec![format!("RENAME TABLE {q} TO {}", renamed(to))],
            "mssql" => {
                let old = object
                    .schema
                    .as_ref()
                    .map_or(object.name.clone(), |s| format!("{s}.{}", object.name));
                vec![format!(
                    "EXEC sp_rename {}, {}",
                    dialect.literal(&crate::Value::Text(old)),
                    dialect.literal(&crate::Value::Text(to.trim().to_string()))
                )]
            }
            "sqlite" if kind != ObjectKind::Table => {
                return Err(Error::Unsupported(
                    "SQLite cannot rename a view; drop it and create it again".into(),
                ))
            }
            // PostgreSQL and SQLite take a bare new name (the schema stays).
            _ => vec![format!(
                "ALTER {noun} {q} RENAME TO {}",
                dialect.quote_ident(to.trim())
            )],
        },
        ObjectOp::Drop => vec![format!("DROP {noun} {q}")],
        ObjectOp::Truncate => {
            only_tables()?;
            match name {
                // SQLite has no TRUNCATE; an unqualified DELETE is its fast path.
                "sqlite" => vec![format!("DELETE FROM {q}")],
                _ => vec![format!("TRUNCATE TABLE {q}")],
            }
        }
        // Handled above: SQL has no bare create.
        ObjectOp::Create => Vec::new(),
        ObjectOp::Duplicate { to, with_data } => {
            only_tables()?;
            let new = renamed(to);
            let copy_rows = format!("INSERT INTO {new} SELECT * FROM {q}");
            match name {
                "postgres" => {
                    let mut v = vec![format!("CREATE TABLE {new} (LIKE {q} INCLUDING ALL)")];
                    if *with_data {
                        v.push(copy_rows);
                    }
                    v
                }
                "mysql" => {
                    let mut v = vec![format!("CREATE TABLE {new} LIKE {q}")];
                    if *with_data {
                        v.push(copy_rows);
                    }
                    v
                }
                "mssql" => vec![format!(
                    "SELECT * INTO {new} FROM {q}{}",
                    if *with_data { "" } else { " WHERE 1 = 0" }
                )],
                // SQLite copies columns and rows, not constraints or indexes.
                _ => vec![format!(
                    "CREATE TABLE {new} AS SELECT * FROM {q}{}",
                    if *with_data { "" } else { " WHERE 0" }
                )],
            }
        }
    })
}

fn mongo(object: &ObjectRef, op: &ObjectOp) -> String {
    let s = |t: &str| crate::doc::Doc::String(t.to_string()).to_pretty();
    let coll = format!("db.getCollection({})", s(&object.name));
    match op {
        ObjectOp::Rename { to } => format!("{coll}.renameCollection({})", s(to.trim())),
        ObjectOp::Drop => format!("{coll}.drop()"),
        ObjectOp::Truncate => format!("{coll}.deleteMany({{}})"),
        ObjectOp::Duplicate {
            to,
            with_data: true,
        } => {
            format!(
                "{coll}.aggregate([{{ $match: {{}} }}, {{ $out: {} }}])",
                s(to.trim())
            )
        }
        ObjectOp::Duplicate {
            to,
            with_data: false,
        } => format!("db.createCollection({})", s(to.trim())),
        ObjectOp::Create => format!("db.createCollection({})", s(object.name.trim())),
    }
}

/// Redis: an object is one key. Renames and copies never overwrite an existing key.
fn redis(object: &ObjectRef, op: &ObjectOp) -> Result<String> {
    let k = crate::cmdline::quote_arg(&object.name);
    let q = |t: &str| crate::cmdline::quote_arg(t.trim());
    Ok(match op {
        ObjectOp::Rename { to } => format!("RENAMENX {k} {}", q(to)),
        ObjectOp::Drop | ObjectOp::Truncate => format!("DEL {k}"),
        ObjectOp::Duplicate {
            to,
            with_data: true,
        } => format!("COPY {k} {}", q(to)),
        ObjectOp::Duplicate {
            with_data: false, ..
        }
        | ObjectOp::Create => {
            return Err(Error::Unsupported(
                "an empty Redis key; add it with a command such as HSET".into(),
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(schema: Option<&str>, name: &str) -> ObjectRef {
        ObjectRef {
            schema: schema.map(str::to_string),
            name: name.into(),
        }
    }

    fn rename(to: &str) -> ObjectOp {
        ObjectOp::Rename { to: to.into() }
    }

    #[test]
    fn postgres() {
        let d = Dialect::POSTGRES;
        let o = t(Some("public"), "users");
        let s = |op| statements(&d, ObjectKind::Table, &o, &op).unwrap();
        assert_eq!(
            s(rename("people")),
            [r#"ALTER TABLE "public"."users" RENAME TO "people""#]
        );
        assert_eq!(s(ObjectOp::Drop), [r#"DROP TABLE "public"."users""#]);
        assert_eq!(
            s(ObjectOp::Truncate),
            [r#"TRUNCATE TABLE "public"."users""#]
        );
        assert_eq!(
            s(ObjectOp::Duplicate {
                to: "users_copy".into(),
                with_data: true
            }),
            [
                r#"CREATE TABLE "public"."users_copy" (LIKE "public"."users" INCLUDING ALL)"#,
                r#"INSERT INTO "public"."users_copy" SELECT * FROM "public"."users""#,
            ]
        );
        assert_eq!(
            statements(&d, ObjectKind::MaterializedView, &o, &ObjectOp::Drop).unwrap(),
            [r#"DROP MATERIALIZED VIEW "public"."users""#]
        );
    }

    #[test]
    fn mysql_sqlite_mssql() {
        let o = t(None, "a`b");
        assert_eq!(
            statements(&Dialect::MYSQL, ObjectKind::Table, &o, &rename("c")).unwrap(),
            ["RENAME TABLE `a``b` TO `c`"]
        );
        let o = t(None, "logs");
        assert_eq!(
            statements(&Dialect::SQLITE, ObjectKind::Table, &o, &ObjectOp::Truncate).unwrap(),
            [r#"DELETE FROM "logs""#]
        );
        assert_eq!(
            statements(
                &Dialect::SQLITE,
                ObjectKind::Table,
                &o,
                &ObjectOp::Duplicate {
                    to: "logs2".into(),
                    with_data: false
                }
            )
            .unwrap(),
            [r#"CREATE TABLE "logs2" AS SELECT * FROM "logs" WHERE 0"#]
        );
        assert!(statements(&Dialect::SQLITE, ObjectKind::View, &o, &rename("v2")).is_err());
        assert_eq!(
            statements(
                &Dialect::MSSQL,
                ObjectKind::Table,
                &t(Some("dbo"), "it's"),
                &rename("x")
            )
            .unwrap(),
            ["EXEC sp_rename 'dbo.it''s', 'x'"]
        );
    }

    #[test]
    fn redis_keys() {
        let o = t(None, "user 1");
        let s = |op| statements(&Dialect::REDIS, ObjectKind::Table, &o, &op);
        assert_eq!(s(ObjectOp::Drop).unwrap(), [r#"DEL "user 1""#]);
        assert_eq!(
            s(ObjectOp::Rename {
                to: "user:1".into()
            })
            .unwrap(),
            [r#"RENAMENX "user 1" user:1"#]
        );
        assert!(s(ObjectOp::Create).is_err());
        assert!(statements(&Dialect::MQTT, ObjectKind::Table, &o, &ObjectOp::Drop).is_err());
    }

    #[test]
    fn mongodb() {
        let o = t(None, "orders");
        let s = |op| statements(&Dialect::MONGODB, ObjectKind::Table, &o, &op).unwrap();
        assert_eq!(s(ObjectOp::Drop), [r#"db.getCollection("orders").drop()"#]);
        assert_eq!(
            s(ObjectOp::Truncate),
            [r#"db.getCollection("orders").deleteMany({})"#]
        );
        assert_eq!(
            s(rename("o2")),
            [r#"db.getCollection("orders").renameCollection("o2")"#]
        );
        assert_eq!(s(ObjectOp::Create), [r#"db.createCollection("orders")"#]);
        assert!(statements(&Dialect::POSTGRES, ObjectKind::Table, &o, &ObjectOp::Create).is_err());
    }

    #[test]
    fn needs_a_new_name_and_a_table() {
        let o = t(None, "v");
        assert!(statements(&Dialect::SQLITE, ObjectKind::Table, &o, &rename(" ")).is_err());
        assert!(statements(&Dialect::SQLITE, ObjectKind::Table, &o, &rename("v")).is_err());
        assert!(statements(
            &Dialect::POSTGRES,
            ObjectKind::View,
            &o,
            &ObjectOp::Truncate
        )
        .is_err());
    }
}
