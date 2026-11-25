//! Changes to a SQL table's structure from the Structure view: add or drop a column,
//! create or drop an index, add or drop a foreign key. Statements are built here through
//! the dialect (identifiers quoted), previewed by the GUI, then run by
//! `Engine::run_structure_op`, which refuses read-only connections.
//!
//! Column types and default expressions are SQL the user typed for this purpose; they
//! are checked for shape (no `;`, no comments) and shown in the preview before running.

use crate::dialect::{Dialect, QueryLanguage};
use crate::schema::ObjectRef;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum StructureOp {
    #[serde(rename_all = "camelCase")]
    AddColumn {
        name: String,
        data_type: String,
        #[serde(default = "yes")]
        nullable: bool,
        /// A default expression as SQL (`0`, `'OPEN'`, `now()`).
        #[serde(default)]
        default: Option<String>,
    },
    DropColumn {
        name: String,
    },
    /// Rename a column and/or change its definition. `from` is the definition it has
    /// now (from `describe_table`): only what differs is changed, and MySQL, which
    /// restates the whole column, keeps the parts not edited. Without `from` every part
    /// of `to` is applied.
    #[serde(rename_all = "camelCase")]
    AlterColumn {
        name: String,
        #[serde(default)]
        rename_to: Option<String>,
        #[serde(default)]
        from: Option<ColumnDef>,
        #[serde(default)]
        to: Option<ColumnDef>,
    },
    CreateIndex {
        name: String,
        columns: Vec<String>,
        #[serde(default)]
        unique: bool,
    },
    DropIndex {
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    AddForeignKey {
        /// Constraint name; generated from the table and columns when empty.
        #[serde(default)]
        name: Option<String>,
        columns: Vec<String>,
        ref_table: ObjectRef,
        ref_columns: Vec<String>,
        #[serde(default)]
        on_update: Option<String>,
        #[serde(default)]
        on_delete: Option<String>,
    },
    DropForeignKey {
        name: String,
    },
}

fn yes() -> bool {
    true
}

/// A column's definition as `AlterColumn` compares and writes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDef {
    pub data_type: String,
    #[serde(default = "yes")]
    pub nullable: bool,
    /// The default as SQL; `None` (or empty) for no default.
    #[serde(default)]
    pub default: Option<String>,
    /// Kept when MySQL restates the column (MODIFY would drop it otherwise).
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub auto_increment: bool,
}

impl ColumnDef {
    /// The definition `describe_table` reports for a column.
    pub fn of(c: &crate::schema::ColumnInfo) -> Self {
        ColumnDef {
            data_type: c.data_type.clone(),
            nullable: c.nullable,
            default: c.default.clone(),
            comment: c.comment.clone(),
            auto_increment: c.auto_increment,
        }
    }

    fn default_sql(&self) -> Option<&str> {
        self.default
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }
}

impl StructureOp {
    /// Short label for titles, status and history ("Add column"…).
    pub fn verb(&self) -> &'static str {
        match self {
            StructureOp::AddColumn { .. } => "Add column",
            StructureOp::DropColumn { .. } => "Drop column",
            StructureOp::AlterColumn { .. } => "Change column",
            StructureOp::CreateIndex { .. } => "Create index",
            StructureOp::DropIndex { .. } => "Drop index",
            StructureOp::AddForeignKey { .. } => "Add relation",
            StructureOp::DropForeignKey { .. } => "Drop relation",
        }
    }

    /// Dropping a column loses its data.
    pub fn destroys_data(&self) -> bool {
        matches!(self, StructureOp::DropColumn { .. })
    }
}

/// Referential actions every SQL database here accepts.
pub const FK_ACTIONS: [&str; 5] = [
    "NO ACTION",
    "RESTRICT",
    "CASCADE",
    "SET NULL",
    "SET DEFAULT",
];

fn required(what: &str, v: &str) -> Result<String> {
    let v = v.trim();
    if v.is_empty() {
        return Err(Error::InvalidParams(format!("{what} is required")));
    }
    Ok(v.to_string())
}

/// A type or default typed by the user: one SQL fragment, never a second statement.
fn fragment(what: &str, v: &str) -> Result<String> {
    let v = required(what, v)?;
    if v.contains(';') || v.contains("--") || v.contains("/*") {
        return Err(Error::InvalidParams(format!(
            "{what} must be a single expression (no `;` or comments)"
        )));
    }
    Ok(v)
}

fn action(what: &str, v: &Option<String>) -> Result<Option<&'static str>> {
    let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    FK_ACTIONS
        .iter()
        .find(|a| a.eq_ignore_ascii_case(v))
        .copied()
        .map(Some)
        .ok_or_else(|| Error::InvalidParams(format!("{what}: unknown action `{v}`")))
}

fn column_list(d: &Dialect, what: &str, cols: &[String]) -> Result<String> {
    let cols: Vec<String> = cols
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| d.quote_ident(c))
        .collect();
    if cols.is_empty() {
        return Err(Error::InvalidParams(format!(
            "{what}: pick at least one column"
        )));
    }
    Ok(cols.join(", "))
}

/// The statements that apply `op` to `table`, in order.
pub fn statements(d: &Dialect, table: &ObjectRef, op: &StructureOp) -> Result<Vec<String>> {
    if d.language != QueryLanguage::Sql {
        return Err(Error::Unsupported(
            "structure changes work on SQL tables only".into(),
        ));
    }
    let t = d.quote_object(table);
    // Objects that live in the table's schema (indexes on PostgreSQL).
    let in_schema = |name: &str| {
        d.quote_object(&ObjectRef {
            schema: table.schema.clone(),
            name: name.to_string(),
        })
    };
    let engine = d.name;
    Ok(match op {
        StructureOp::AddColumn {
            name,
            data_type,
            nullable,
            default,
        } => {
            let mut s = format!(
                "ALTER TABLE {t} ADD COLUMN {} {}",
                d.quote_ident(&required("A column name", name)?),
                fragment("A type", data_type)?
            );
            if !nullable {
                s.push_str(" NOT NULL");
            }
            if let Some(def) = default.as_deref().filter(|v| !v.trim().is_empty()) {
                s.push_str(&format!(" DEFAULT {}", fragment("The default", def)?));
            }
            vec![s]
        }
        StructureOp::DropColumn { name } => vec![format!(
            "ALTER TABLE {t} DROP COLUMN {}",
            d.quote_ident(&required("A column name", name)?)
        )],
        StructureOp::AlterColumn {
            name,
            rename_to,
            from,
            to,
        } => alter_column(d, table, &t, name, rename_to, from, to)?,
        StructureOp::CreateIndex {
            name,
            columns,
            unique,
        } => vec![format!(
            "CREATE {}INDEX {} ON {t} ({})",
            if *unique { "UNIQUE " } else { "" },
            d.quote_ident(&required("An index name", name)?),
            column_list(d, "Index", columns)?
        )],
        StructureOp::DropIndex { name } => {
            let name = required("An index name", name)?;
            vec![match engine {
                "mysql" => format!("DROP INDEX {} ON {t}", d.quote_ident(&name)),
                "mssql" => format!("DROP INDEX {} ON {t}", d.quote_ident(&name)),
                _ => format!("DROP INDEX {}", in_schema(&name)),
            }]
        }
        StructureOp::AddForeignKey {
            name,
            columns,
            ref_table,
            ref_columns,
            on_update,
            on_delete,
        } => {
            if engine == "sqlite" {
                return Err(Error::Unsupported(
                    "SQLite cannot add a foreign key to an existing table; \
                     it has to be in CREATE TABLE"
                        .into(),
                ));
            }
            let cols = column_list(d, "Relation", columns)?;
            let refs = column_list(d, "Referenced columns", ref_columns)?;
            if columns.iter().filter(|c| !c.trim().is_empty()).count()
                != ref_columns.iter().filter(|c| !c.trim().is_empty()).count()
            {
                return Err(Error::InvalidParams(
                    "a relation needs as many referenced columns as columns".into(),
                ));
            }
            let name = match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(n) => n.to_string(),
                None => format!("fk_{}_{}", table.name, columns.join("_")),
            };
            required("A referenced table", &ref_table.name)?;
            let mut s = format!(
                "ALTER TABLE {t} ADD CONSTRAINT {} FOREIGN KEY ({cols}) REFERENCES {} ({refs})",
                d.quote_ident(&name),
                d.quote_object(ref_table)
            );
            if let Some(a) = action("On update", on_update)? {
                s.push_str(&format!(" ON UPDATE {a}"));
            }
            if let Some(a) = action("On delete", on_delete)? {
                s.push_str(&format!(" ON DELETE {a}"));
            }
            vec![s]
        }
        StructureOp::DropForeignKey { name } => {
            if engine == "sqlite" {
                return Err(Error::Unsupported(
                    "SQLite cannot drop a foreign key; the table has to be rebuilt".into(),
                ));
            }
            let name = d.quote_ident(&required("A relation name", name)?);
            vec![match engine {
                "mysql" => format!("ALTER TABLE {t} DROP FOREIGN KEY {name}"),
                _ => format!("ALTER TABLE {t} DROP CONSTRAINT {name}"),
            }]
        }
    })
}

/// Statements for `AlterColumn`: the rename first, then the definition changes on the
/// new name.
fn alter_column(
    d: &Dialect,
    table: &ObjectRef,
    t: &str,
    name: &str,
    rename_to: &Option<String>,
    from: &Option<ColumnDef>,
    to: &Option<ColumnDef>,
) -> Result<Vec<String>> {
    let name = required("A column name", name)?;
    let mut out = Vec::new();
    let mut col = name.clone();
    if let Some(new) = rename_to
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        if new != name {
            out.push(match d.name {
                "mssql" => {
                    let old = format!(
                        "{}.{}",
                        table
                            .schema
                            .as_deref()
                            .map_or(table.name.clone(), |s| format!("{s}.{}", table.name)),
                        name
                    );
                    format!(
                        "EXEC sp_rename {}, {}, 'COLUMN'",
                        d.literal(&crate::Value::Text(old)),
                        d.literal(&crate::Value::Text(new.to_string()))
                    )
                }
                _ => format!(
                    "ALTER TABLE {t} RENAME COLUMN {} TO {}",
                    d.quote_ident(&name),
                    d.quote_ident(new)
                ),
            });
            col = new.to_string();
        }
    }
    if let Some(to) = to {
        let data_type = fragment("A type", &to.data_type)?;
        let new_default = match to.default_sql() {
            Some(v) => Some(fragment("The default", v)?),
            None => None,
        };
        let type_changed = from
            .as_ref()
            .is_none_or(|f| !f.data_type.trim().eq_ignore_ascii_case(&data_type));
        let null_changed = from.as_ref().is_none_or(|f| f.nullable != to.nullable);
        let default_changed = from
            .as_ref()
            .is_none_or(|f| f.default_sql() != new_default.as_deref());
        let c = d.quote_ident(&col);
        match d.name {
            "sqlite" => {
                if type_changed || null_changed || default_changed {
                    return Err(Error::Unsupported(
                        "SQLite can only rename a column; changing its type, NULL or \
                         default means rebuilding the table"
                            .into(),
                    ));
                }
            }
            "mysql" => {
                // MODIFY restates the whole column, so it carries what was not edited.
                if type_changed || null_changed {
                    let mut s = format!("ALTER TABLE {t} MODIFY COLUMN {c} {data_type}");
                    if !to.nullable {
                        s.push_str(" NOT NULL");
                    }
                    if let Some(v) = &new_default {
                        s.push_str(&format!(" DEFAULT {v}"));
                    }
                    if to.auto_increment {
                        s.push_str(" AUTO_INCREMENT");
                    }
                    if let Some(cm) = to.comment.as_deref().filter(|c| !c.is_empty()) {
                        s.push_str(&format!(
                            " COMMENT {}",
                            d.literal(&crate::Value::Text(cm.to_string()))
                        ));
                    }
                    out.push(s);
                } else if default_changed {
                    out.push(match &new_default {
                        Some(v) => format!("ALTER TABLE {t} ALTER COLUMN {c} SET DEFAULT {v}"),
                        None => format!("ALTER TABLE {t} ALTER COLUMN {c} DROP DEFAULT"),
                    });
                }
            }
            "mssql" => {
                if default_changed {
                    return Err(Error::Unsupported(
                        "SQL Server keeps defaults as named constraints; change them in SQL".into(),
                    ));
                }
                if type_changed || null_changed {
                    out.push(format!(
                        "ALTER TABLE {t} ALTER COLUMN {c} {data_type} {}",
                        if to.nullable { "NULL" } else { "NOT NULL" }
                    ));
                }
            }
            // PostgreSQL and the like: one clause per part that changed.
            _ => {
                if type_changed {
                    out.push(format!(
                        "ALTER TABLE {t} ALTER COLUMN {c} TYPE {data_type} USING {c}::{data_type}"
                    ));
                }
                if null_changed {
                    out.push(format!(
                        "ALTER TABLE {t} ALTER COLUMN {c} {} NOT NULL",
                        if to.nullable { "DROP" } else { "SET" }
                    ));
                }
                if default_changed {
                    out.push(match &new_default {
                        Some(v) => format!("ALTER TABLE {t} ALTER COLUMN {c} SET DEFAULT {v}"),
                        None => format!("ALTER TABLE {t} ALTER COLUMN {c} DROP DEFAULT"),
                    });
                }
            }
        }
    }
    if out.is_empty() {
        return Err(Error::InvalidParams("nothing changed".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shifts() -> ObjectRef {
        ObjectRef {
            schema: Some("public".into()),
            name: "shifts".into(),
        }
    }

    #[test]
    fn postgres_statements() {
        let d = Dialect::POSTGRES;
        let one = |op: StructureOp| statements(&d, &shifts(), &op).unwrap().join("; ");
        assert_eq!(
            one(StructureOp::AddColumn {
                name: "note".into(),
                data_type: "varchar(40)".into(),
                nullable: false,
                default: Some("''".into()),
            }),
            r#"ALTER TABLE "public"."shifts" ADD COLUMN "note" varchar(40) NOT NULL DEFAULT ''"#
        );
        assert_eq!(
            one(StructureOp::CreateIndex {
                name: "idx_shop".into(),
                columns: vec!["shop_id".into(), "status".into()],
                unique: true,
            }),
            r#"CREATE UNIQUE INDEX "idx_shop" ON "public"."shifts" ("shop_id", "status")"#
        );
        assert_eq!(
            one(StructureOp::DropIndex {
                name: "idx_shop".into()
            }),
            r#"DROP INDEX "public"."idx_shop""#
        );
        assert_eq!(
            one(StructureOp::AddForeignKey {
                name: None,
                columns: vec!["shop_id".into()],
                ref_table: ObjectRef {
                    schema: Some("public".into()),
                    name: "shops".into()
                },
                ref_columns: vec!["id".into()],
                on_update: None,
                on_delete: Some("cascade".into()),
            }),
            r#"ALTER TABLE "public"."shifts" ADD CONSTRAINT "fk_shifts_shop_id" FOREIGN KEY ("shop_id") REFERENCES "public"."shops" ("id") ON DELETE CASCADE"#
        );
        assert_eq!(
            one(StructureOp::DropForeignKey {
                name: "fk_shifts_shop_id".into()
            }),
            r#"ALTER TABLE "public"."shifts" DROP CONSTRAINT "fk_shifts_shop_id""#
        );
    }

    #[test]
    fn mysql_and_sqlite_differences() {
        let t = ObjectRef::new("shifts");
        let m = Dialect::MYSQL;
        assert_eq!(
            statements(&m, &t, &StructureOp::DropIndex { name: "i".into() }).unwrap(),
            vec!["DROP INDEX `i` ON `shifts`"]
        );
        assert_eq!(
            statements(&m, &t, &StructureOp::DropForeignKey { name: "f".into() }).unwrap(),
            vec!["ALTER TABLE `shifts` DROP FOREIGN KEY `f`"]
        );
        let s = Dialect::SQLITE;
        assert!(matches!(
            statements(&s, &t, &StructureOp::DropForeignKey { name: "f".into() }),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(
            statements(&s, &t, &StructureOp::DropColumn { name: "x".into() }).unwrap(),
            vec![r#"ALTER TABLE "shifts" DROP COLUMN "x""#]
        );
    }

    #[test]
    fn alter_column_changes_only_what_differs() {
        let was = ColumnDef {
            data_type: "integer".into(),
            nullable: true,
            default: None,
            comment: Some("count".into()),
            auto_increment: false,
        };
        let alter = |d: &Dialect, rename: Option<&str>, to: ColumnDef| {
            statements(
                d,
                &ObjectRef::new("t"),
                &StructureOp::AlterColumn {
                    name: "n".into(),
                    rename_to: rename.map(String::from),
                    from: Some(was.clone()),
                    to: Some(to),
                },
            )
        };
        let pg = Dialect::POSTGRES;
        // Rename only.
        assert_eq!(
            alter(&pg, Some("qty"), was.clone()).unwrap(),
            vec![r#"ALTER TABLE "t" RENAME COLUMN "n" TO "qty""#]
        );
        // New type and NOT NULL, on the new name; the default is untouched.
        assert_eq!(
            alter(
                &pg,
                Some("qty"),
                ColumnDef {
                    data_type: "bigint".into(),
                    nullable: false,
                    ..was.clone()
                }
            )
            .unwrap(),
            vec![
                r#"ALTER TABLE "t" RENAME COLUMN "n" TO "qty""#,
                r#"ALTER TABLE "t" ALTER COLUMN "qty" TYPE bigint USING "qty"::bigint"#,
                r#"ALTER TABLE "t" ALTER COLUMN "qty" SET NOT NULL"#,
            ]
        );
        // MySQL restates the column and keeps its comment.
        assert_eq!(
            alter(
                &Dialect::MYSQL,
                None,
                ColumnDef {
                    data_type: "bigint".into(),
                    ..was.clone()
                }
            )
            .unwrap(),
            vec!["ALTER TABLE `t` MODIFY COLUMN `n` bigint COMMENT 'count'"]
        );
        // A default alone on MySQL does not restate the column.
        assert_eq!(
            alter(
                &Dialect::MYSQL,
                None,
                ColumnDef {
                    default: Some("0".into()),
                    ..was.clone()
                }
            )
            .unwrap(),
            vec!["ALTER TABLE `t` ALTER COLUMN `n` SET DEFAULT 0"]
        );
        // SQLite renames but refuses a new type; nothing changed is an error.
        let lite = Dialect::SQLITE;
        assert!(alter(&lite, Some("qty"), was.clone()).is_ok());
        assert!(matches!(
            alter(
                &lite,
                None,
                ColumnDef {
                    data_type: "text".into(),
                    ..was.clone()
                }
            ),
            Err(Error::Unsupported(_))
        ));
        assert!(alter(&pg, None, was.clone()).is_err());
    }

    #[test]
    fn refuses_bad_input() {
        let d = Dialect::POSTGRES;
        let t = ObjectRef::new("shifts");
        let add = |ty: &str, def: Option<&str>| {
            statements(
                &d,
                &t,
                &StructureOp::AddColumn {
                    name: "c".into(),
                    data_type: ty.into(),
                    nullable: true,
                    default: def.map(String::from),
                },
            )
        };
        assert!(add("int; DROP TABLE x", None).is_err());
        assert!(add("int", Some("1 -- x")).is_err());
        assert!(add("", None).is_err());
        assert!(statements(
            &d,
            &t,
            &StructureOp::CreateIndex {
                name: "i".into(),
                columns: vec![],
                unique: false
            }
        )
        .is_err());
        assert!(statements(
            &d,
            &t,
            &StructureOp::AddForeignKey {
                name: None,
                columns: vec!["a".into()],
                ref_table: ObjectRef::new("b"),
                ref_columns: vec!["id".into()],
                on_update: Some("EXPLODE".into()),
                on_delete: None
            }
        )
        .is_err());
    }
}
