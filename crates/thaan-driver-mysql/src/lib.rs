//! MySQL / MariaDB driver. Written from the MySQL reference manual (client/server
//! protocol, `information_schema`, `SHOW CREATE …`).
//!
//! Statements without parameters use the text protocol (every statement works there);
//! statements with parameters are prepared and bound (binary protocol).

use mysql::consts::{ColumnFlags, ColumnType};
use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder, Params};
use std::time::Duration;
use thaan_core::connection::ConnectionConfig;
use thaan_core::dialect::Dialect;
use thaan_core::driver::{Driver, DriverInfo, ParamKind, ParamSpec, Session};
use thaan_core::schema::{
    ColumnInfo, ForeignKey, IndexInfo, ObjectKind, ObjectRef, SchemaObject, TableInfo, TriggerInfo,
};
use thaan_core::{Column, Error, Result, ResultSet, Value};

/// Character set number MySQL uses for binary strings.
const BINARY_CHARSET: u16 = 63;
const SYSTEM_SCHEMAS: &str = "('mysql','information_schema','performance_schema','sys')";

pub struct MysqlDriver;

impl Driver for MysqlDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mysql".into(),
            name: "MySQL / MariaDB".into(),
            params: vec![
                ParamSpec::new("host", "Host", ParamKind::Text).default("localhost"),
                ParamSpec::new("port", "Port", ParamKind::Number).default("3306"),
                ParamSpec::new("user", "User", ParamKind::Text).placeholder("root"),
                ParamSpec::new("password", "Password", ParamKind::Password),
                ParamSpec::new("database", "Database", ParamKind::Text).placeholder("(optional)"),
            ],
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::MYSQL
    }

    fn connect(&self, c: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let port: u16 = c
            .param("port")
            .unwrap_or("3306")
            .parse()
            .map_err(|_| Error::InvalidParams("port must be a number".into()))?;
        let opts = OptsBuilder::new()
            .ip_or_hostname(Some(c.param("host").unwrap_or("localhost")))
            .tcp_port(port)
            .user(Some(c.param("user").unwrap_or("root")))
            .pass(c.param("password"))
            .db_name(c.param("database"))
            .tcp_connect_timeout(Some(Duration::from_secs(10)))
            .prefer_socket(false);
        let mut conn = Conn::new(opts).map_err(db)?;
        if c.read_only {
            conn.query_drop("SET SESSION TRANSACTION READ ONLY")
                .map_err(db)?;
        }
        Ok(Box::new(MysqlSession { conn }))
    }
}

fn db(e: mysql::Error) -> Error {
    match e {
        mysql::Error::MySqlError(m) => {
            Error::Database(format!("{} ({}): {}", m.code, m.state, m.message))
        }
        other => Error::Database(other.to_string()),
    }
}

fn to_mysql(v: &Value) -> mysql::Value {
    match v {
        Value::Null => mysql::Value::NULL,
        Value::Bool(b) => mysql::Value::Int(*b as i64),
        Value::Int(i) => mysql::Value::Int(*i),
        Value::Float(f) => mysql::Value::Double(*f),
        Value::Text(s) => mysql::Value::Bytes(s.as_bytes().to_vec()),
        Value::Bytes(b) => mysql::Value::Bytes(b.clone()),
    }
}

fn type_name(t: ColumnType) -> &'static str {
    use ColumnType::*;
    match t {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_INT24 => "mediumint",
        MYSQL_TYPE_LONG => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_YEAR => "year",
        MYSQL_TYPE_BIT => "bit",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_ENUM => "enum",
        MYSQL_TYPE_SET => "set",
        MYSQL_TYPE_GEOMETRY => "geometry",
        MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB | MYSQL_TYPE_BLOB => {
            "blob"
        }
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => "varchar",
        MYSQL_TYPE_STRING => "char",
        _ => "unknown",
    }
}

struct ColInfo {
    ty: ColumnType,
    binary: bool,
    unsigned: bool,
}

/// Converts one protocol value into our `Value`, using the column metadata.
fn from_mysql(v: mysql::Value, c: &ColInfo) -> Value {
    use ColumnType::*;
    match v {
        mysql::Value::NULL => Value::Null,
        mysql::Value::Int(i) => Value::Int(i),
        mysql::Value::UInt(u) => i64::try_from(u)
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(u.to_string())),
        mysql::Value::Float(f) => Value::Float(f as f64),
        mysql::Value::Double(f) => Value::Float(f),
        mysql::Value::Date(y, mo, d, h, mi, s, us) => {
            if matches!(c.ty, MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE) {
                Value::Text(format!("{y:04}-{mo:02}-{d:02}"))
            } else if us > 0 {
                Value::Text(format!(
                    "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}"
                ))
            } else {
                Value::Text(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"))
            }
        }
        mysql::Value::Time(neg, days, h, mi, s, us) => {
            let sign = if neg { "-" } else { "" };
            let hours = days as u64 * 24 + h as u64;
            let frac = if us > 0 {
                format!(".{us:06}")
            } else {
                String::new()
            };
            Value::Text(format!("{sign}{hours:02}:{mi:02}:{s:02}{frac}"))
        }
        mysql::Value::Bytes(b) => {
            let as_text = || String::from_utf8_lossy(&b).into_owned();
            match c.ty {
                MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG
                | MYSQL_TYPE_YEAR => as_text()
                    .parse()
                    .map(Value::Int)
                    .unwrap_or_else(|_| Value::Text(as_text())),
                MYSQL_TYPE_LONGLONG if !c.unsigned => as_text()
                    .parse()
                    .map(Value::Int)
                    .unwrap_or_else(|_| Value::Text(as_text())),
                MYSQL_TYPE_LONGLONG => as_text()
                    .parse::<i64>()
                    .map(Value::Int)
                    .unwrap_or_else(|_| Value::Text(as_text())),
                MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => as_text()
                    .parse()
                    .map(Value::Float)
                    .unwrap_or_else(|_| Value::Text(as_text())),
                MYSQL_TYPE_BIT => {
                    if b.len() <= 8 {
                        Value::Int(b.iter().fold(0i64, |acc, x| (acc << 8) | *x as i64))
                    } else {
                        Value::Bytes(b)
                    }
                }
                _ if c.binary => Value::Bytes(b),
                _ => Value::Text(as_text()),
            }
        }
    }
}

pub struct MysqlSession {
    conn: Conn,
}

impl MysqlSession {
    fn collect<T: mysql::prelude::Protocol>(
        mut qr: mysql::QueryResult<'_, '_, '_, T>,
    ) -> Result<ResultSet> {
        let cols: Vec<mysql::Column> = qr.columns().as_ref().to_vec();
        if cols.is_empty() {
            let n = qr.affected_rows();
            return Ok(ResultSet {
                rows_affected: Some(n),
                ..Default::default()
            });
        }
        let info: Vec<ColInfo> = cols
            .iter()
            .map(|c| ColInfo {
                ty: c.column_type(),
                // Dates and numbers also report the binary charset in the text
                // protocol; only string and blob columns can hold raw bytes.
                binary: c.character_set() == BINARY_CHARSET
                    && matches!(
                        c.column_type(),
                        ColumnType::MYSQL_TYPE_VARCHAR
                            | ColumnType::MYSQL_TYPE_VAR_STRING
                            | ColumnType::MYSQL_TYPE_STRING
                            | ColumnType::MYSQL_TYPE_TINY_BLOB
                            | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
                            | ColumnType::MYSQL_TYPE_LONG_BLOB
                            | ColumnType::MYSQL_TYPE_BLOB
                            | ColumnType::MYSQL_TYPE_GEOMETRY
                    ),
                unsigned: c.flags().contains(ColumnFlags::UNSIGNED_FLAG),
            })
            .collect();
        let columns = cols
            .iter()
            .map(|c| Column {
                name: c.name_str().into_owned(),
                type_name: Some(type_name(c.column_type()).into()),
            })
            .collect();
        let mut rows = Vec::new();
        for row in qr.by_ref() {
            let row = row.map_err(db)?;
            rows.push(
                row.unwrap()
                    .into_iter()
                    .zip(&info)
                    .map(|(v, c)| from_mysql(v, c))
                    .collect(),
            );
        }
        Ok(ResultSet {
            columns,
            rows,
            ..Default::default()
        })
    }

    fn run(&mut self, sql: &str, params: &[Value]) -> Result<ResultSet> {
        if params.is_empty() {
            let qr = self.conn.query_iter(sql).map_err(db)?;
            Self::collect(qr)
        } else {
            let p = Params::Positional(params.iter().map(to_mysql).collect());
            let qr = self.conn.exec_iter(sql, p).map_err(db)?;
            Self::collect(qr)
        }
    }

    fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>> {
        Ok(self.run(sql, params)?.rows)
    }

    fn current_db(&mut self) -> Result<Option<String>> {
        Ok(self
            .rows("SELECT DATABASE()", &[])?
            .first()
            .and_then(|r| match &r[0] {
                Value::Null => None,
                v => Some(text(v)),
            }))
    }

    /// The schema an object lives in: its own, else the session's current database.
    fn schema_of(&mut self, o: &ObjectRef) -> Result<String> {
        match &o.schema {
            Some(s) => Ok(s.clone()),
            None => self.current_db()?.ok_or_else(|| {
                Error::InvalidParams(
                    "no database selected; qualify the object with a schema".into(),
                )
            }),
        }
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        other => other.to_string(),
    }
}

fn as_int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        other => text(other).parse().unwrap_or(0),
    }
}

impl Session for MysqlSession {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<ResultSet> {
        self.run(sql, params)
    }

    fn begin(&mut self) -> Result<()> {
        self.run("START TRANSACTION", &[]).map(|_| ())
    }

    /// MySQL calls them schemas; they play the part of databases in the tree.
    fn list_databases(&mut self) -> Result<Vec<String>> {
        Ok(self
            .rows("SHOW DATABASES", &[])?
            .iter()
            .map(|r| text(&r[0]))
            .collect())
    }

    fn current_database(&mut self) -> Result<Option<String>> {
        self.current_db()
    }

    fn list_objects(&mut self) -> Result<Vec<SchemaObject>> {
        let current = self.current_db()?;
        // With a database selected, list only that one and leave objects unqualified.
        let (filter, params, qualify) = match &current {
            Some(d) => ("= ?".to_string(), vec![Value::Text(d.clone())], false),
            None => (format!("NOT IN {SYSTEM_SCHEMAS}"), vec![], true),
        };
        let obj = |schema: &Value, name: &Value| ObjectRef {
            schema: qualify.then(|| text(schema)),
            name: text(name),
        };
        let rows = self.rows(
            &format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA {filter} ORDER BY TABLE_SCHEMA, TABLE_NAME"
            ),
            &params,
        )?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let kind = if text(&r[2]).contains("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            out.push(SchemaObject {
                object: obj(&r[0], &r[1]),
                kind,
                parent: None,
            });
        }
        for r in self.rows(
            &format!(
                "SELECT DISTINCT TABLE_SCHEMA, INDEX_NAME, TABLE_NAME FROM information_schema.STATISTICS \
                 WHERE TABLE_SCHEMA {filter} ORDER BY TABLE_SCHEMA, INDEX_NAME"
            ),
            &params,
        )? {
            out.push(SchemaObject { object: obj(&r[0], &r[1]), kind: ObjectKind::Index, parent: Some(text(&r[2])) });
        }
        for r in self.rows(
            &format!(
                "SELECT TRIGGER_SCHEMA, TRIGGER_NAME, EVENT_OBJECT_TABLE FROM information_schema.TRIGGERS \
                 WHERE TRIGGER_SCHEMA {filter} ORDER BY 1, 2"
            ),
            &params,
        )? {
            out.push(SchemaObject { object: obj(&r[0], &r[1]), kind: ObjectKind::Trigger, parent: Some(text(&r[2])) });
        }
        for r in self.rows(
            &format!(
                "SELECT ROUTINE_SCHEMA, ROUTINE_NAME FROM information_schema.ROUTINES \
                 WHERE ROUTINE_SCHEMA {filter} ORDER BY 1, 2"
            ),
            &params,
        )? {
            out.push(SchemaObject {
                object: obj(&r[0], &r[1]),
                kind: ObjectKind::Routine,
                parent: None,
            });
        }
        Ok(out)
    }

    fn describe_table(&mut self, t: &ObjectRef) -> Result<TableInfo> {
        let key = [Value::Text(self.schema_of(t)?), Value::Text(t.name.clone())];
        let pk: Vec<(String, u32)> = self
            .rows(
                "SELECT COLUMN_NAME, ORDINAL_POSITION FROM information_schema.KEY_COLUMN_USAGE \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY'",
                &key,
            )?
            .iter()
            .map(|r| (text(&r[0]), as_int(&r[1]) as u32))
            .collect();
        let cols = self.rows(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT, EXTRA \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            &key,
        )?;
        if cols.is_empty() {
            return Err(Error::Database(format!("table {} does not exist", t.name)));
        }
        let columns = cols
            .iter()
            .map(|r| {
                let name = text(&r[0]);
                let (data_type, extra) = (text(&r[1]), text(&r[5]).to_ascii_lowercase());
                ColumnInfo {
                    primary_key: pk.iter().find(|(n, _)| *n == name).map(|(_, p)| *p),
                    name,
                    nullable: text(&r[2]) == "YES",
                    default: (!r[3].is_null())
                        .then(|| default_sql(&text(&r[3]), &data_type, &extra)),
                    data_type,
                    // MySQL stores "no comment" as an empty string.
                    comment: Some(text(&r[4])).filter(|c| !c.is_empty()),
                    auto_increment: extra.contains("auto_increment"),
                }
            })
            .collect();
        let mut foreign_keys: Vec<(String, ForeignKey)> = Vec::new();
        for r in self.rows(
            "SELECT k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_NAME, \
                    k.REFERENCED_COLUMN_NAME, k.REFERENCED_TABLE_SCHEMA, \
                    rc.UPDATE_RULE, rc.DELETE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k \
             LEFT JOIN information_schema.REFERENTIAL_CONSTRAINTS rc \
               ON rc.CONSTRAINT_SCHEMA = k.TABLE_SCHEMA AND rc.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
             WHERE k.TABLE_SCHEMA = ? AND k.TABLE_NAME = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
            &key,
        )? {
            let name = text(&r[0]);
            match foreign_keys.iter_mut().find(|(n, _)| *n == name) {
                Some((_, fk)) => {
                    fk.columns.push(text(&r[1]));
                    fk.ref_columns.push(text(&r[3]));
                }
                None => foreign_keys.push((
                    name.clone(),
                    ForeignKey {
                        name: Some(name),
                        columns: vec![text(&r[1])],
                        ref_schema: Some(text(&r[4])),
                        ref_table: text(&r[2]),
                        ref_columns: vec![text(&r[3])],
                        on_update: (!r[5].is_null()).then(|| text(&r[5])),
                        on_delete: (!r[6].is_null()).then(|| text(&r[6])),
                    },
                )),
            }
        }
        let mut indexes: Vec<IndexInfo> = Vec::new();
        for r in self.rows(
            "SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX",
            &key,
        )? {
            let name = text(&r[0]);
            let col = text(&r[2]);
            match indexes.iter_mut().find(|i| i.name == name) {
                Some(i) => i.columns.push(col),
                None => indexes.push(IndexInfo {
                    primary: name == "PRIMARY",
                    name,
                    columns: vec![col],
                    unique: as_int(&r[1]) == 0,
                }),
            }
        }
        let triggers = self
            .rows(
                "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, ACTION_STATEMENT \
                 FROM information_schema.TRIGGERS \
                 WHERE EVENT_OBJECT_SCHEMA = ? AND EVENT_OBJECT_TABLE = ? ORDER BY TRIGGER_NAME",
                &key,
            )?
            .iter()
            .map(|r| TriggerInfo {
                name: text(&r[0]),
                timing: text(&r[1]),
                event: text(&r[2]),
                action: text(&r[3]),
                // MySQL has no WHEN clause; conditions live inside the body.
                condition: None,
            })
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
        let schema = self.schema_of(o)?;
        let key = [Value::Text(schema.clone()), Value::Text(o.name.clone())];
        let d = Dialect::MYSQL;
        let qualified = format!("{}.{}", d.quote_ident(&schema), d.quote_ident(&o.name));
        // Identifiers are quoted by the dialect, so the SHOW statements are safe to build.
        let show = |what: &str| format!("SHOW CREATE {what} {qualified}");
        let is =
            |this: &mut Self, sql: &str| -> Result<bool> { Ok(!this.rows(sql, &key)?.is_empty()) };
        let (stmt, col) = if is(self, "SELECT 1 FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?")? {
            (show("TABLE"), 1)
        } else if is(self, "SELECT 1 FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ? AND TRIGGER_NAME = ?")? {
            (show("TRIGGER"), 2)
        } else {
            let kind = self
                .rows("SELECT ROUTINE_TYPE FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = ? AND ROUTINE_NAME = ?", &key)?
                .first()
                .map(|r| text(&r[0]))
                .ok_or_else(|| Error::Database(format!("no such object: {qualified}")))?;
            (show(&kind), 2)
        };
        let rs = self.run(&stmt, &[])?;
        let row = rs
            .rows
            .first()
            .ok_or_else(|| Error::Database(format!("no definition for {qualified}")))?;
        Ok(format!("{};", text(row.get(col).unwrap_or(&Value::Null))))
    }
}

#[cfg(test)]
mod tests {
    //! Live tests need a server: `THAAN_TEST_MYSQL_URL=mysql://user:pw@host:port/db`.
    use super::*;
    use std::sync::Arc;
    use thaan_core::engine::{Engine, QueryRequest, RunScope};
    use thaan_core::store::Store;
    use thaan_core::table::{PageRequest, StagedChange};

    fn info(ty: ColumnType) -> ColInfo {
        ColInfo {
            ty,
            binary: false,
            unsigned: false,
        }
    }

    #[test]
    fn value_conversion() {
        assert_eq!(
            from_mysql(
                mysql::Value::Bytes(b"42".to_vec()),
                &info(ColumnType::MYSQL_TYPE_LONG)
            ),
            Value::Int(42)
        );
        assert_eq!(
            from_mysql(
                mysql::Value::Bytes(b"1.5".to_vec()),
                &info(ColumnType::MYSQL_TYPE_DOUBLE)
            ),
            Value::Float(1.5)
        );
        assert_eq!(
            from_mysql(
                mysql::Value::Bytes(b"12.30".to_vec()),
                &info(ColumnType::MYSQL_TYPE_NEWDECIMAL)
            ),
            Value::from("12.30")
        );
        assert_eq!(
            from_mysql(
                mysql::Value::Bytes(vec![1, 2]),
                &info(ColumnType::MYSQL_TYPE_BIT)
            ),
            Value::Int(258)
        );
        assert_eq!(
            from_mysql(
                mysql::Value::Date(2024, 2, 29, 0, 0, 0, 0),
                &info(ColumnType::MYSQL_TYPE_DATE)
            ),
            Value::from("2024-02-29")
        );
        assert_eq!(
            from_mysql(
                mysql::Value::Date(2024, 2, 29, 13, 5, 1, 250_000),
                &info(ColumnType::MYSQL_TYPE_DATETIME)
            ),
            Value::from("2024-02-29 13:05:01.250000")
        );
        assert_eq!(
            from_mysql(
                mysql::Value::Time(true, 1, 2, 3, 4, 0),
                &info(ColumnType::MYSQL_TYPE_TIME)
            ),
            Value::from("-26:03:04")
        );
        assert_eq!(
            from_mysql(
                mysql::Value::UInt(u64::MAX),
                &info(ColumnType::MYSQL_TYPE_LONGLONG)
            ),
            Value::from(u64::MAX.to_string())
        );
        let bin = ColInfo {
            ty: ColumnType::MYSQL_TYPE_BLOB,
            binary: true,
            unsigned: false,
        };
        assert_eq!(
            from_mysql(mysql::Value::Bytes(vec![0, 255]), &bin),
            Value::Bytes(vec![0, 255])
        );
    }

    #[test]
    #[ignore = "needs THAAN_TEST_MYSQL_URL"]
    fn live_roundtrip() {
        let Ok(url) = std::env::var("THAAN_TEST_MYSQL_URL") else {
            return;
        };
        let e = Engine::new(Store::in_memory());
        e.register_driver(Arc::new(MysqlDriver));
        let sid = e
            .open(ConnectionConfig::from_url(&url).unwrap())
            .unwrap()
            .session_id;
        let run = |sql: &str, params: std::collections::BTreeMap<String, Value>| {
            e.run_query(&QueryRequest {
                session_id: sid,
                sql: sql.into(),
                scope: RunScope::All,
                params,
            })
            .unwrap()
        };
        let r = run(
            "DROP TABLE IF EXISTS thaan_posts; DROP TABLE IF EXISTS thaan_users; DROP VIEW IF EXISTS thaan_v;
             CREATE TABLE thaan_users (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(50) NOT NULL,
               score DECIMAL(10,3), born DATE, seen DATETIME(3), meta JSON, big BIGINT UNSIGNED,
               raw VARBINARY(4), ok TINYINT(1) DEFAULT 1, UNIQUE KEY u_name (name));
             CREATE TABLE thaan_posts (id INT PRIMARY KEY, user_id INT, title TEXT,
               CONSTRAINT fk_user FOREIGN KEY (user_id) REFERENCES thaan_users(id));
             CREATE VIEW thaan_v AS SELECT id, name FROM thaan_users;
             INSERT INTO thaan_users (name, score, born, seen, meta, big, raw) VALUES
               ('ann', -1234.5, '1999-12-31', '2024-02-29 13:45:01.250', '{\"k\": [1, 2]}', 18446744073709551615, X'00FF'),
               ('bob', 0.001, NULL, NULL, NULL, 1, NULL);",
            Default::default(),
        );
        assert!(r.iter().all(|x| x.error.is_none()), "{r:?}");
        assert_eq!(
            r.last().unwrap().result.as_ref().unwrap().rows_affected,
            Some(2)
        );

        let r = run(
            "SELECT name, score, born, seen, meta, big, raw, ok FROM thaan_users ORDER BY id",
            Default::default(),
        );
        let rs = r[0].result.as_ref().unwrap();
        assert_eq!(
            rs.rows[0],
            vec![
                Value::from("ann"),
                Value::from("-1234.500"),
                Value::from("1999-12-31"),
                Value::from("2024-02-29 13:45:01.250"),
                Value::from("{\"k\": [1, 2]}"),
                Value::from("18446744073709551615"),
                Value::Bytes(vec![0, 255]),
                Value::Int(1),
            ]
        );

        // Prepared (binary protocol) path.
        let mut p = std::collections::BTreeMap::new();
        p.insert("n".to_string(), Value::from("ann"));
        let r = run("SELECT id, born, seen FROM thaan_users WHERE name = :n", p);
        assert_eq!(
            r[0].result.as_ref().unwrap().rows[0],
            vec![
                Value::Int(1),
                Value::from("1999-12-31"),
                Value::from("2024-02-29 13:45:01.250000")
            ]
        );

        let objs = e.list_objects(sid).unwrap();
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::View && o.object.name == "thaan_v"));
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::Index && o.object.name == "u_name"));
        let t = ObjectRef::new("thaan_users");
        let info = e.describe_table(sid, &t).unwrap();
        assert_eq!(info.primary_key(), ["id"]);
        assert!(!info.columns[1].nullable);
        let posts = e
            .describe_table(sid, &ObjectRef::new("thaan_posts"))
            .unwrap();
        assert_eq!(posts.foreign_keys[0].ref_table, "thaan_users");
        assert!(e
            .ddl(sid, &t)
            .unwrap()
            .starts_with("CREATE TABLE `thaan_users`"));
        assert!(e
            .ddl(sid, &ObjectRef::new("thaan_v"))
            .unwrap()
            .contains("VIEW"));

        let page = e
            .fetch_page(
                sid,
                &PageRequest {
                    table: t.clone(),
                    filters: vec![],
                    raw_where: None,
                    projection: None,
                    raw_sort: None,
                    sort: vec![],
                    page: 0,
                    page_size: 1,
                },
            )
            .unwrap();
        assert_eq!(page.total_rows, 2);
        let n = e
            .apply_changes(
                sid,
                &t,
                &[StagedChange::Update {
                    key: vec![("id".into(), Value::Int(2))],
                    set: vec![("score".into(), Value::Float(9.5))],
                }],
            )
            .unwrap();
        assert_eq!(n, 1);
        run(
            "DROP TABLE thaan_posts; DROP TABLE thaan_users; DROP VIEW thaan_v",
            Default::default(),
        );
    }
}

/// A column default as SQL. MySQL reports a literal default bare (`OPEN` for `'OPEN'`)
/// and an expression default with `DEFAULT_GENERATED` in EXTRA; MariaDB quotes literals
/// itself. Restating a column (MODIFY) needs the SQL form.
fn default_sql(raw: &str, data_type: &str, extra: &str) -> String {
    let ty = data_type.to_ascii_lowercase();
    let numeric = [
        "int", "decimal", "numeric", "float", "double", "real", "bit", "bool",
    ]
    .iter()
    .any(|n| ty.contains(n));
    let is_number = raw.parse::<f64>().is_ok();
    if extra.contains("default_generated") || raw.starts_with('\'') || (numeric && is_number) {
        raw.to_string()
    } else {
        Dialect::MYSQL.literal(&Value::Text(raw.to_string()))
    }
}

#[cfg(test)]
mod default_tests {
    use super::default_sql;

    #[test]
    fn defaults_become_sql() {
        assert_eq!(default_sql("OPEN", "varchar(10)", ""), "'OPEN'");
        assert_eq!(default_sql("0.00", "decimal(12,2)", ""), "0.00");
        assert_eq!(
            default_sql("CURRENT_TIMESTAMP", "timestamp", "default_generated"),
            "CURRENT_TIMESTAMP"
        );
        assert_eq!(default_sql("'x'", "varchar(3)", ""), "'x'");
        assert_eq!(default_sql("it's", "text", ""), "'it''s'");
    }
}
