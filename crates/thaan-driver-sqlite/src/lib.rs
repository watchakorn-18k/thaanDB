//! SQLite driver, written from the SQLite documentation (sqlite.org/lang.html,
//! sqlite.org/pragma.html, sqlite.org/schematab.html).

use rusqlite::types::ValueRef;
use rusqlite::{params_from_iter, Connection, OpenFlags};
use thaan_core::connection::ConnectionConfig;
use thaan_core::dialect::Dialect;
use thaan_core::driver::{Driver, DriverInfo, ParamKind, ParamSpec, Session};
use thaan_core::schema::{
    ColumnInfo, ForeignKey, IndexInfo, ObjectKind, ObjectRef, SchemaObject, TableInfo, TriggerInfo,
};
use thaan_core::{Column, Error, Result, ResultSet, Value};

pub struct SqliteDriver;

impl Driver for SqliteDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "sqlite".into(),
            name: "SQLite".into(),
            params: vec![ParamSpec::new("path", "Database file", ParamKind::File)
                .required()
                .placeholder("/path/to/app.db or :memory:")],
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::SQLITE
    }

    fn connect(&self, c: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let path = c
            .param("path")
            .ok_or_else(|| Error::InvalidParams("`path` is required".into()))?;
        let conn = if path == ":memory:" {
            Connection::open_in_memory()
        } else {
            let mut flags = OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            flags |= if c.read_only {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
            };
            Connection::open_with_flags(path, flags)
        }
        .map_err(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db)?;
        conn.execute_batch("PRAGMA foreign_keys = ON").map_err(db)?;
        if c.read_only {
            conn.execute_batch("PRAGMA query_only = ON").map_err(db)?;
        }
        Ok(Box::new(SqliteSession { conn }))
    }
}

pub struct SqliteSession {
    conn: Connection,
}

#[allow(clippy::needless_pass_by_value)] // by value so it can be `map_err(db)`
fn db(e: rusqlite::Error) -> Error {
    Error::Database(e.to_string())
}

fn to_sql(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(*b as i64),
        Value::Int(i) => V::Integer(*i),
        Value::Float(f) => V::Real(*f),
        Value::Text(s) => V::Text(s.clone()),
        Value::Bytes(b) => V::Blob(b.clone()),
    }
}

fn from_sql(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::Bytes(b.to_vec()),
    }
}

impl SqliteSession {
    fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet> {
        let mut stmt = self.conn.prepare(sql).map_err(db)?;
        let columns: Vec<Column> = stmt
            .columns()
            .map(|c| Column {
                name: c.name().to_string(),
                type_name: c.decl_type().map(str::to_string),
            })
            .collect();
        let bound = params_from_iter(params.iter().map(to_sql));
        if columns.is_empty() {
            let n = stmt.execute(bound).map_err(db)?;
            return Ok(ResultSet {
                rows_affected: Some(n as u64),
                ..Default::default()
            });
        }
        let ncol = columns.len();
        let mut rows = stmt.query(bound).map_err(db)?;
        let mut out = Vec::new();
        while let Some(r) = rows.next().map_err(db)? {
            let mut row = Vec::with_capacity(ncol);
            for i in 0..ncol {
                row.push(from_sql(r.get_ref(i).map_err(db)?));
            }
            out.push(row);
        }
        drop(rows);
        // RETURNING clauses produce rows *and* change rows.
        let affected = if stmt.readonly() {
            None
        } else {
            Some(self.conn.changes())
        };
        Ok(ResultSet {
            columns,
            rows: out,
            rows_affected: affected,
            ..Default::default()
        })
    }

    fn texts(&self, sql: &str, p: &[Value]) -> Result<Vec<Vec<Value>>> {
        Ok(self.query(sql, p)?.rows)
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl Session for SqliteSession {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<ResultSet> {
        self.query(sql, params)
    }

    /// `main` plus any attached databases.
    fn list_databases(&mut self) -> Result<Vec<String>> {
        Ok(self
            .texts(
                "SELECT name FROM pragma_database_list WHERE name <> 'temp'",
                &[],
            )?
            .iter()
            .map(|r| text(&r[0]))
            .collect())
    }

    fn current_database(&mut self) -> Result<Option<String>> {
        Ok(Some("main".into()))
    }

    fn list_objects(&mut self) -> Result<Vec<SchemaObject>> {
        let rows = self.texts(
            "SELECT type, name, tbl_name FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name COLLATE NOCASE",
            &[],
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let kind = match text(&r[0]).as_str() {
                    "table" => ObjectKind::Table,
                    "view" => ObjectKind::View,
                    "index" => ObjectKind::Index,
                    "trigger" => ObjectKind::Trigger,
                    _ => return None,
                };
                let name = text(&r[1]);
                let parent =
                    matches!(kind, ObjectKind::Index | ObjectKind::Trigger).then(|| text(&r[2]));
                Some(SchemaObject {
                    object: ObjectRef::new(name),
                    kind,
                    parent,
                })
            })
            .collect())
    }

    fn describe_table(&mut self, t: &ObjectRef) -> Result<TableInfo> {
        let name = Value::Text(t.name.clone());
        let cols = self.texts(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?)",
            std::slice::from_ref(&name),
        )?;
        if cols.is_empty() {
            return Err(Error::Database(format!("no such table: {}", t.name)));
        }
        let columns = cols
            .iter()
            .map(|r| ColumnInfo {
                name: text(&r[0]),
                data_type: text(&r[1]),
                nullable: r[2] != Value::Int(1),
                default: (!r[3].is_null()).then(|| text(&r[3])),
                primary_key: match r[4] {
                    Value::Int(n) if n > 0 => Some(n as u32),
                    _ => None,
                },
                comment: None,
                auto_increment: false,
            })
            .collect();

        let fk_rows = self.texts(
            "SELECT id, \"table\", \"from\", \"to\", on_update, on_delete \
             FROM pragma_foreign_key_list(?) ORDER BY id, seq",
            std::slice::from_ref(&name),
        )?;
        let mut foreign_keys: Vec<(i64, ForeignKey)> = Vec::new();
        for r in fk_rows {
            let id = if let Value::Int(i) = r[0] { i } else { 0 };
            match foreign_keys.iter_mut().find(|(i, _)| *i == id) {
                Some((_, fk)) => {
                    fk.columns.push(text(&r[2]));
                    fk.ref_columns.push(text(&r[3]));
                }
                None => foreign_keys.push((
                    id,
                    ForeignKey {
                        name: None,
                        columns: vec![text(&r[2])],
                        ref_schema: None,
                        ref_table: text(&r[1]),
                        ref_columns: vec![text(&r[3])],
                        on_update: Some(text(&r[4])),
                        on_delete: Some(text(&r[5])),
                    },
                )),
            }
        }

        let idx_rows = self.texts(
            "SELECT name, \"unique\", origin FROM pragma_index_list(?)",
            std::slice::from_ref(&name),
        )?;
        let mut indexes = Vec::new();
        for r in idx_rows {
            let iname = text(&r[0]);
            let cols = self.texts(
                "SELECT name FROM pragma_index_info(?) ORDER BY seqno",
                &[Value::Text(iname.clone())],
            )?;
            indexes.push(IndexInfo {
                name: iname,
                columns: cols.iter().map(|c| text(&c[0])).collect(),
                unique: r[1] == Value::Int(1),
                // `pk`: the index SQLite made for a PRIMARY KEY constraint.
                primary: text(&r[2]) == "pk",
            });
        }

        let trigger_rows = self.texts(
            "SELECT name, sql FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ? \
             ORDER BY name",
            std::slice::from_ref(&name),
        )?;
        let triggers = trigger_rows
            .iter()
            .map(|r| parse_trigger(text(&r[0]), &text(&r[1])))
            .collect();

        Ok(TableInfo {
            object: t.clone(),
            columns,
            foreign_keys: foreign_keys.into_iter().map(|(_, f)| f).collect(),
            indexes,
            triggers,
        })
    }

    fn ddl(&mut self, o: &ObjectRef) -> Result<String> {
        let rows = self.texts(
            "SELECT sql FROM sqlite_schema WHERE name = ?",
            &[Value::Text(o.name.clone())],
        )?;
        let mut parts: Vec<String> = rows
            .iter()
            .map(|r| text(&r[0]))
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return Err(Error::Database(format!("no such object: {}", o.name)));
        }
        // Include indexes and triggers of a table.
        let extra = self.texts(
            "SELECT sql FROM sqlite_schema WHERE tbl_name = ? AND name <> ? AND sql IS NOT NULL ORDER BY type, name",
            &[Value::Text(o.name.clone()), Value::Text(o.name.clone())],
        )?;
        parts.extend(extra.iter().map(|r| text(&r[0])));
        Ok(parts.join(";\n\n") + ";")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use thaan_core::engine::{Engine, QueryRequest, RunScope};
    use thaan_core::export::{ExportFormat, ExportOptions};
    use thaan_core::store::Store;
    use thaan_core::table::{Filter, FilterOp, PageRequest, StagedChange};

    fn engine() -> (Engine, u64) {
        let e = Engine::new(Store::in_memory());
        e.register_driver(Arc::new(SqliteDriver));
        let s = e
            .open(ConnectionConfig::from_url("sqlite::memory:").unwrap())
            .unwrap();
        let req = QueryRequest {
            session_id: s.session_id,
            sql: "create table users (id integer primary key, name text not null, age int default 0);\
                  create index users_name on users(name);\
                  create table posts (id integer primary key, user_id int references users(id), title text);\
                  insert into users (name, age) values ('ann', 31), ('bob', 17), ('cy', 45);"
                .into(),
            scope: RunScope::All,
            params: Default::default(),
        };
        let r = e.run_query(&req).unwrap();
        assert!(r.iter().all(|x| x.error.is_none()), "{r:?}");
        (e, s.session_id)
    }

    #[test]
    fn transfer_between_databases() {
        use std::sync::atomic::AtomicBool;
        use thaan_core::schema::ObjectRef;
        use thaan_core::transfer::{ExistingTable, TransferContent, TransferRequest};
        let (e, source) = engine();
        // 1,203 rows so the copy takes several batches.
        let more: Vec<String> = (0..1200).map(|i| format!("('u{i}', {i})")).collect();
        e.run_query(&QueryRequest {
            session_id: source,
            sql: format!("insert into users (name, age) values {}", more.join(",")),
            scope: RunScope::All,
            params: Default::default(),
        })
        .unwrap();
        let target = e
            .open(ConnectionConfig::from_url("sqlite::memory:").unwrap())
            .unwrap()
            .session_id;
        let count = |sid, t: &str| {
            let r = e
                .run_query(&QueryRequest {
                    session_id: sid,
                    sql: format!("select count(*) from {t}"),
                    scope: RunScope::All,
                    params: Default::default(),
                })
                .unwrap();
            r[0].result.as_ref().map(|rs| rs.rows[0][0].clone())
        };
        let users = ObjectRef {
            schema: None,
            name: "users".into(),
        };
        let mut req = TransferRequest {
            source,
            target,
            tables: vec![users.clone()],
            content: TransferContent::StructureAndData,
            existing: ExistingTable::Append,
            batch_size: 500,
        };
        let seen = std::cell::RefCell::new(Vec::new());
        let never = AtomicBool::new(false);
        let r = e
            .transfer(&req, &|p| seen.borrow_mut().push(p.rows_done), &never)
            .unwrap();
        assert_eq!((r.tables, r.rows), (1, 1203));
        assert_eq!(*seen.borrow(), vec![500, 1000, 1203]);
        assert_eq!(count(target, "users"), Some(Value::Int(1203)));
        // Truncate empties first; Replace rebuilds; Append adds to what is there.
        req.content = TransferContent::DataOnly;
        req.existing = ExistingTable::Truncate;
        e.transfer(&req, &|_| {}, &never).unwrap();
        assert_eq!(count(target, "users"), Some(Value::Int(1203)));
        req.content = TransferContent::StructureOnly;
        req.existing = ExistingTable::Replace;
        e.transfer(&req, &|_| {}, &never).unwrap();
        assert_eq!(count(target, "users"), Some(Value::Int(0)));
        req.content = TransferContent::DataOnly;
        req.existing = ExistingTable::Append;
        e.transfer(&req, &|_| {}, &never).unwrap();
        assert_eq!(count(target, "users"), Some(Value::Int(1203)));
        // Appending the same keys again stops with the database's error.
        assert!(e.transfer(&req, &|_| {}, &never).is_err());
        req.content = TransferContent::StructureOnly;
        req.existing = ExistingTable::Replace;
        e.transfer(&req, &|_| {}, &never).unwrap();
        // Cancelled before it starts: nothing copied.
        req.content = TransferContent::StructureAndData;
        req.existing = ExistingTable::Append;
        let r = e.transfer(&req, &|_| {}, &AtomicBool::new(true)).unwrap();
        assert!(r.cancelled);
        assert_eq!(count(target, "users"), Some(Value::Int(0)));
        // Data only into a table the target does not have is refused.
        req.content = TransferContent::DataOnly;
        req.tables = vec![ObjectRef {
            schema: None,
            name: "posts".into(),
        }];
        assert!(e.transfer(&req, &|_| {}, &never).is_err());
    }

    #[test]
    fn object_ops_rename_duplicate_truncate_drop() {
        use thaan_core::object_ops::ObjectOp;
        use thaan_core::schema::{ObjectKind, ObjectRef};
        let (e, sid) = engine();
        let t = |n: &str| ObjectRef {
            schema: None,
            name: n.into(),
        };
        let count = |table: &str| {
            let r = e
                .run_query(&QueryRequest {
                    session_id: sid,
                    sql: format!("select count(*) from {table}"),
                    scope: RunScope::All,
                    params: Default::default(),
                })
                .unwrap();
            r[0].result.as_ref().map(|rs| rs.rows[0][0].clone())
        };
        let run = |o: &str, op: ObjectOp| e.run_object_op(sid, ObjectKind::Table, &t(o), &op);
        run(
            "users",
            ObjectOp::Rename {
                to: "people".into(),
            },
        )
        .unwrap();
        assert_eq!(count("people"), Some(Value::Int(3)));
        run(
            "people",
            ObjectOp::Duplicate {
                to: "people_copy".into(),
                with_data: true,
            },
        )
        .unwrap();
        assert_eq!(count("people_copy"), Some(Value::Int(3)));
        run("people_copy", ObjectOp::Truncate).unwrap();
        assert_eq!(count("people_copy"), Some(Value::Int(0)));
        run("people_copy", ObjectOp::Drop).unwrap();
        assert_eq!(count("people_copy"), None);
        // A failing statement is reported, not swallowed.
        assert!(run("missing", ObjectOp::Drop).is_err());
    }

    #[test]
    fn object_ops_refused_on_read_only() {
        use thaan_core::object_ops::ObjectOp;
        use thaan_core::schema::{ObjectKind, ObjectRef};
        let e = Engine::new(Store::in_memory());
        e.register_driver(Arc::new(SqliteDriver));
        let mut c = ConnectionConfig::from_url("sqlite::memory:").unwrap();
        c.read_only = true;
        let s = e.open(c).unwrap();
        let t = ObjectRef {
            schema: None,
            name: "x".into(),
        };
        let err = e
            .run_object_op(s.session_id, ObjectKind::Table, &t, &ObjectOp::Drop)
            .unwrap_err();
        assert!(matches!(err, thaan_core::Error::ReadOnly(_)), "{err}");
    }

    #[test]
    fn run_query_and_params() {
        let (e, sid) = engine();
        let mut params = std::collections::BTreeMap::new();
        params.insert("min".to_string(), Value::Int(18));
        let r = e
            .run_query(&QueryRequest {
                session_id: sid,
                sql: "select name from users where age >= :min order by name".into(),
                scope: RunScope::All,
                params,
            })
            .unwrap();
        let rs = r[0].result.as_ref().unwrap();
        assert_eq!(
            rs.rows,
            vec![vec![Value::from("ann")], vec![Value::from("cy")]]
        );
        assert_eq!(e.history(10).len(), 5);
    }

    #[test]
    fn stops_at_first_error() {
        let (e, sid) = engine();
        let r = e
            .run_query(&QueryRequest {
                session_id: sid,
                sql: "select 1; select * from nope; select 2".into(),
                scope: RunScope::All,
                params: Default::default(),
            })
            .unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[1].error.as_ref().unwrap().contains("no such table"));
    }

    #[test]
    fn run_current_statement() {
        let (e, sid) = engine();
        let sql = "select 1 as a;\nselect 2 as b;";
        let r = e
            .run_query(&QueryRequest {
                session_id: sid,
                sql: sql.into(),
                scope: RunScope::Current { cursor: 17 },
                params: Default::default(),
            })
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].result.as_ref().unwrap().columns[0].name, "b");
    }

    #[test]
    fn schema_introspection() {
        let (e, sid) = engine();
        let objs = e.list_objects(sid).unwrap();
        let names: Vec<_> = objs
            .iter()
            .map(|o| (o.kind, o.object.name.as_str()))
            .collect();
        assert!(names.contains(&(ObjectKind::Table, "users")));
        assert!(names.contains(&(ObjectKind::Index, "users_name")));
        let t = e.describe_table(sid, &ObjectRef::new("users")).unwrap();
        assert_eq!(t.primary_key(), ["id"]);
        assert!(!t.columns[1].nullable);
        assert_eq!(t.columns[2].default.as_deref(), Some("0"));
        assert_eq!(t.indexes[0].columns, ["name"]);
        let p = e.describe_table(sid, &ObjectRef::new("posts")).unwrap();
        assert_eq!(p.foreign_keys[0].ref_table, "users");
        let ddl = e.ddl(sid, &ObjectRef::new("users")).unwrap();
        assert!(ddl.contains("CREATE INDEX users_name") || ddl.contains("create index users_name"));
    }

    #[test]
    fn paging_filter_and_changes() {
        let (e, sid) = engine();
        let req = PageRequest {
            table: ObjectRef::new("users"),
            filters: vec![Filter {
                column: "age".into(),
                op: FilterOp::Ge,
                values: vec![Value::Int(18)],
            }],
            raw_where: None,
            projection: None,
            raw_sort: None,
            sort: vec![],
            page: 0,
            page_size: 1,
        };
        let page = e.fetch_page(sid, &req).unwrap();
        assert_eq!(page.total_rows, 2);
        assert_eq!(page.page_count(), 2);
        assert_eq!(page.result.rows.len(), 1);

        let t = ObjectRef::new("users");
        let changes = vec![
            StagedChange::Update {
                key: vec![("id".into(), Value::Int(2))],
                set: vec![("age".into(), Value::Int(18))],
            },
            StagedChange::Insert {
                values: vec![("name".into(), "dee".into())],
            },
            StagedChange::Delete {
                key: vec![("id".into(), Value::Int(3))],
            },
        ];
        assert_eq!(
            e.preview_changes(sid, &t, &changes).unwrap()[0],
            "UPDATE \"users\" SET \"age\" = 18 WHERE \"id\" = 2"
        );
        assert_eq!(e.apply_changes(sid, &t, &changes).unwrap(), 3);
        let page = e
            .fetch_page(
                sid,
                &PageRequest {
                    page_size: 100,
                    ..req.clone()
                },
            )
            .unwrap();
        assert_eq!(page.total_rows, 2); // ann, bob(18); cy deleted; dee age 0

        // A failing change rolls the whole batch back.
        let bad = vec![
            StagedChange::Update {
                key: vec![("id".into(), Value::Int(1))],
                set: vec![("age".into(), Value::Int(99))],
            },
            StagedChange::Insert {
                values: vec![("name".into(), Value::Null)],
            },
        ];
        assert!(e.apply_changes(sid, &t, &bad).is_err());
        let csv = e
            .export_table(
                sid,
                &PageRequest {
                    filters: vec![],
                    ..req
                },
                &ExportOptions::new(ExportFormat::Csv),
            )
            .unwrap();
        assert!(csv.contains("1,ann,31"), "{csv}");
    }

    #[test]
    fn read_only_is_enforced() {
        let dir =
            std::env::temp_dir().join(format!("thaan-sqlite-{}", thaan_core::store::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let e = Engine::new(Store::in_memory());
        e.register_driver(Arc::new(SqliteDriver));
        let mut c = ConnectionConfig::from_url(&format!("sqlite:{}", path.display())).unwrap();
        let rw = e.open(c.clone()).unwrap();
        let q = |sid, sql: &str| {
            e.run_query(&QueryRequest {
                session_id: sid,
                sql: sql.into(),
                scope: RunScope::All,
                params: Default::default(),
            })
            .unwrap()
        };
        assert!(q(rw.session_id, "create table t(a)")[0].error.is_none());
        c.read_only = true;
        let ro = e.open(c).unwrap();
        assert!(q(ro.session_id, "select * from t")[0].error.is_none());
        let err = q(ro.session_id, "insert into t values (1)")[0]
            .error
            .clone()
            .unwrap();
        assert!(err.contains("read-only"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }
}

/// Splits a stored `CREATE TRIGGER` statement into timing, event, condition and body.
/// SQLite keeps only the statement text, so this reads its keywords; an unusual
/// statement still shows its name and the whole text as the action.
fn parse_trigger(name: String, sql: &str) -> TriggerInfo {
    let upper = sql.to_ascii_uppercase();
    // Timing and event sit before `ON <table>`; the body may use the same words.
    let head = &upper[..upper.find(" ON ").unwrap_or(upper.len())];
    let words: Vec<&str> = head.split_whitespace().collect();
    let has = |w: &str| words.contains(&w);
    let timing = if head.contains("INSTEAD OF") {
        "INSTEAD OF"
    } else if has("AFTER") {
        "AFTER"
    } else {
        "BEFORE"
    };
    let event = ["INSERT", "UPDATE", "DELETE"]
        .into_iter()
        .find(|e| has(e))
        .unwrap_or("")
        .to_string();
    // Byte offsets line up: ASCII upper-casing keeps every char's length.
    let begin = upper.find("BEGIN");
    let action = match (begin, upper.rfind("END")) {
        (Some(b), Some(e)) if e > b => sql[b + 5..e].trim().to_string(),
        _ => sql.trim().to_string(),
    };
    let condition = upper.find(" WHEN ").and_then(|w| {
        let end = begin.filter(|b| *b > w)?;
        Some(sql[w + 6..end].trim().to_string()).filter(|c| !c.is_empty())
    });
    TriggerInfo {
        name,
        timing: timing.to_string(),
        event,
        action,
        condition,
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::parse_trigger;

    #[test]
    fn reads_trigger_parts() {
        let t = parse_trigger(
            "audit".into(),
            "CREATE TRIGGER audit AFTER UPDATE OF total ON orders FOR EACH ROW \
             WHEN new.total > 100 BEGIN INSERT INTO log VALUES (new.id); END",
        );
        assert_eq!(t.timing, "AFTER");
        assert_eq!(t.event, "UPDATE");
        assert_eq!(t.condition.as_deref(), Some("new.total > 100"));
        assert_eq!(t.action, "INSERT INTO log VALUES (new.id);");

        let t = parse_trigger(
            "v".into(),
            "CREATE TRIGGER v INSTEAD OF DELETE ON vw BEGIN SELECT 1; END",
        );
        assert_eq!(
            (t.timing.as_str(), t.event.as_str()),
            ("INSTEAD OF", "DELETE")
        );
        assert_eq!(t.condition, None);
    }
}
