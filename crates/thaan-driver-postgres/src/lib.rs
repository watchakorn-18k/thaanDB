//! PostgreSQL driver. Written from the PostgreSQL documentation (system catalogs,
//! frontend/backend protocol, binary send/receive formats of the built-in types).
//!
//! Parameters are sent in *text* format so the server casts them to whatever type the
//! statement expects. Results arrive in binary format and are decoded in [`decode`].

mod decode;
mod tls;

use bytes::BytesMut;
use postgres::types::{to_sql_checked, Format, FromSql, IsNull, ToSql, Type};
use postgres::Client;
use std::time::Duration;
use thaan_core::connection::ConnectionConfig;
use thaan_core::dialect::Dialect;
use thaan_core::driver::{Driver, DriverInfo, ParamKind, ParamSpec, Session};
use thaan_core::schema::{
    ColumnInfo, ForeignKey, IndexInfo, ObjectKind, ObjectRef, SchemaObject, TableInfo, TriggerInfo,
};
use thaan_core::{Column, Error, Result, ResultSet, Value};

pub struct PostgresDriver;

impl Driver for PostgresDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "postgres".into(),
            name: "PostgreSQL".into(),
            params: vec![
                ParamSpec::new("host", "Host", ParamKind::Text).default("localhost"),
                ParamSpec::new("port", "Port", ParamKind::Number).default("5432"),
                ParamSpec::new("user", "User", ParamKind::Text).placeholder("postgres"),
                ParamSpec::new("password", "Password", ParamKind::Password),
                ParamSpec::new("database", "Database", ParamKind::Text).placeholder("postgres"),
                ParamSpec::new("sslmode", "SSL mode", ParamKind::Choice)
                    .choices(tls::MODES)
                    .default("prefer"),
                ParamSpec::new("sslrootcert", "CA certificate", ParamKind::File)
                    .placeholder("PEM file, for verify-ca / verify-full (optional)"),
            ],
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::POSTGRES
    }

    fn connect(&self, c: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let root_cert = c.param("sslrootcert");
        let (ssl_mode, verify) = tls::parse_mode(c.param("sslmode"), root_cert)?;
        let mut cfg = postgres::Config::new();
        cfg.host(c.param("host").unwrap_or("localhost"))
            .port(
                c.param("port")
                    .unwrap_or("5432")
                    .parse()
                    .map_err(|_| Error::InvalidParams("port must be a number".into()))?,
            )
            .user(c.param("user").unwrap_or("postgres"))
            .dbname(c.param("database").unwrap_or("postgres"))
            .application_name("thaandb")
            .connect_timeout(Duration::from_secs(10))
            .ssl_mode(ssl_mode);
        if let Some(pw) = c.param("password") {
            cfg.password(pw);
        }
        let mut client = cfg
            .connect(tls::connector(verify, root_cert)?)
            .map_err(db)?;
        if c.read_only {
            client
                .batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
                .map_err(db)?;
        }
        Ok(Box::new(PgSession { client }))
    }
}

#[allow(clippy::needless_pass_by_value)] // by value so it can be `map_err(db)`
fn db(e: postgres::Error) -> Error {
    // Prefer the server's message ("relation \"x\" does not exist") over the wrapper text.
    match e.as_db_error() {
        Some(d) => Error::Database(match d.detail() {
            Some(detail) => format!("{}: {} ({detail})", d.severity(), d.message()),
            None => format!("{}: {}", d.severity(), d.message()),
        }),
        None => {
            // Wrapper errors ("error performing TLS handshake") hide the reason in their
            // source chain ("invalid peer certificate: UnknownIssuer"); show it.
            let mut msg = e.to_string();
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                let t = s.to_string();
                if !msg.contains(&t) {
                    msg.push_str(": ");
                    msg.push_str(&t);
                }
                src = s.source();
            }
            Error::Database(msg)
        }
    }
}

/// A bound value sent in text format; the server parses it as the parameter's type.
#[derive(Debug)]
struct TextParam(Value);

impl ToSql for TextParam {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> std::result::Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        let text = match &self.0 {
            Value::Null => return Ok(IsNull::Yes),
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Text(s) => s.clone(),
            Value::Bytes(b) => format!("\\x{}", decode::hex(b)),
        };
        out.extend_from_slice(text.as_bytes());
        Ok(IsNull::No)
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

/// Any column, decoded from its binary representation.
struct Raw(Value);

impl<'a> FromSql<'a> for Raw {
    fn from_sql(
        ty: &Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Raw(decode::value(ty, raw)))
    }

    fn from_sql_null(
        _: &Type,
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Raw(Value::Null))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

pub struct PgSession {
    client: Client,
}

impl PgSession {
    fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>> {
        Ok(self.query(sql, params)?.rows)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<ResultSet> {
        let stmt = self.client.prepare(sql).map_err(db)?;
        if stmt.params().len() != params.len() {
            return Err(Error::InvalidParams(format!(
                "statement expects {} parameter(s), got {}",
                stmt.params().len(),
                params.len()
            )));
        }
        let owned: Vec<TextParam> = params.iter().cloned().map(TextParam).collect();
        let refs: Vec<&(dyn ToSql + Sync)> =
            owned.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
        if stmt.columns().is_empty() {
            let n = self.client.execute(&stmt, &refs).map_err(db)?;
            return Ok(ResultSet {
                rows_affected: Some(n),
                ..Default::default()
            });
        }
        let columns: Vec<Column> = stmt
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().to_string(),
                type_name: Some(c.type_().name().to_string()),
            })
            .collect();
        let rows = self.client.query(&stmt, &refs).map_err(db)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let mut row = Vec::with_capacity(columns.len());
            for i in 0..columns.len() {
                let Raw(v) = r.try_get::<_, Raw>(i).map_err(db)?;
                row.push(v);
            }
            out.push(row);
        }
        Ok(ResultSet {
            columns,
            rows: out,
            ..Default::default()
        })
    }

    fn qualified(&self, o: &ObjectRef) -> Value {
        Value::Text(Dialect::POSTGRES.quote_object(o))
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn split_list(v: &Value) -> Vec<String> {
    text(v)
        .split('\u{1f}')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

const USER_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";

impl Session for PgSession {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<ResultSet> {
        self.query(sql, params)
    }

    fn list_databases(&mut self) -> Result<Vec<String>> {
        Ok(self
            .rows(
                "SELECT datname FROM pg_database WHERE datallowconn AND NOT datistemplate ORDER BY 1",
                &[],
            )?
            .iter()
            .map(|r| text(&r[0]))
            .collect())
    }

    fn current_database(&mut self) -> Result<Option<String>> {
        Ok(self
            .rows("SELECT current_database()", &[])?
            .first()
            .map(|r| text(&r[0])))
    }

    fn list_objects(&mut self) -> Result<Vec<SchemaObject>> {
        let mut out = Vec::new();
        let rels = self.rows(
            &format!(
                "SELECT n.nspname, c.relname, c.relkind::text, coalesce(t.relname, '') \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 LEFT JOIN pg_index i ON i.indexrelid = c.oid \
                 LEFT JOIN pg_class t ON t.oid = i.indrelid \
                 WHERE c.relkind IN ('r','p','v','m','i','S','f') AND {USER_SCHEMAS} \
                 ORDER BY 1, 2"
            ),
            &[],
        )?;
        for r in rels {
            let kind = match text(&r[2]).as_str() {
                "r" | "p" | "f" => ObjectKind::Table,
                "v" => ObjectKind::View,
                "m" => ObjectKind::MaterializedView,
                "i" => ObjectKind::Index,
                "S" => ObjectKind::Sequence,
                _ => continue,
            };
            let parent = (kind == ObjectKind::Index).then(|| text(&r[3]));
            out.push(SchemaObject {
                object: ObjectRef {
                    schema: Some(text(&r[0])),
                    name: text(&r[1]),
                },
                kind,
                parent,
            });
        }
        let triggers = self.rows(
            &format!(
                "SELECT n.nspname, tg.tgname, c.relname FROM pg_trigger tg \
                 JOIN pg_class c ON c.oid = tg.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT tg.tgisinternal AND {USER_SCHEMAS} ORDER BY 1, 2"
            ),
            &[],
        )?;
        out.extend(triggers.iter().map(|r| SchemaObject {
            object: ObjectRef {
                schema: Some(text(&r[0])),
                name: text(&r[1]),
            },
            kind: ObjectKind::Trigger,
            parent: Some(text(&r[2])),
        }));
        let routines = self.rows(
            &format!(
                "SELECT DISTINCT n.nspname, p.proname FROM pg_proc p \
                 JOIN pg_namespace n ON n.oid = p.pronamespace \
                 LEFT JOIN pg_depend d ON d.objid = p.oid AND d.deptype = 'e' \
                 WHERE d.objid IS NULL AND {USER_SCHEMAS} ORDER BY 1, 2"
            ),
            &[],
        )?;
        out.extend(routines.iter().map(|r| SchemaObject {
            object: ObjectRef {
                schema: Some(text(&r[0])),
                name: text(&r[1]),
            },
            kind: ObjectKind::Routine,
            parent: None,
        }));
        Ok(out)
    }

    fn describe_table(&mut self, t: &ObjectRef) -> Result<TableInfo> {
        let q = self.qualified(t);
        if self
            .rows("SELECT to_regclass($1)::text", std::slice::from_ref(&q))?
            .first()
            .is_none_or(|r| r[0].is_null())
        {
            return Err(Error::Database(format!(
                "relation {} does not exist",
                text(&q)
            )));
        }
        let cols = self.rows(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, \
                    pg_get_expr(d.adbin, d.adrelid), \
                    (SELECT k.ord FROM pg_index i, \
                       unnest(i.indkey::int2[]) WITH ORDINALITY k(attnum, ord) \
                      WHERE i.indrelid = a.attrelid AND i.indisprimary AND k.attnum = a.attnum), \
                    col_description(a.attrelid, a.attnum) \
             FROM pg_attribute a \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = $1::regclass AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            std::slice::from_ref(&q),
        )?;
        let columns = cols
            .iter()
            .map(|r| ColumnInfo {
                name: text(&r[0]),
                data_type: text(&r[1]),
                nullable: r[2] == Value::Bool(true),
                default: (!r[3].is_null()).then(|| text(&r[3])),
                primary_key: match r[4] {
                    Value::Int(n) if n > 0 => Some(n as u32),
                    _ => None,
                },
                comment: (!r[5].is_null()).then(|| text(&r[5])),
                auto_increment: false,
            })
            .collect();
        let fks = self.rows(
            "SELECT (SELECT string_agg(a.attname, chr(31) ORDER BY k.ord) \
                       FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord) \
                       JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
                    rc.relname, \
                    (SELECT string_agg(a.attname, chr(31) ORDER BY k.ord) \
                       FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord) \
                       JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum), \
                    con.conname, rn.nspname, \
                    con.confupdtype::text, con.confdeltype::text \
             FROM pg_constraint con \
             JOIN pg_class rc ON rc.oid = con.confrelid \
             JOIN pg_namespace rn ON rn.oid = rc.relnamespace \
             WHERE con.conrelid = $1::regclass AND con.contype = 'f' \
             ORDER BY con.conname",
            std::slice::from_ref(&q),
        )?;
        let foreign_keys = fks
            .iter()
            .map(|r| ForeignKey {
                name: Some(text(&r[3])),
                columns: split_list(&r[0]),
                ref_schema: Some(text(&r[4])),
                ref_table: text(&r[1]),
                ref_columns: split_list(&r[2]),
                on_update: Some(fk_action(&text(&r[5])).to_string()),
                on_delete: Some(fk_action(&text(&r[6])).to_string()),
            })
            .collect();
        let idx = self.rows(
            "SELECT ic.relname, i.indisunique, \
                    (SELECT string_agg(pg_get_indexdef(i.indexrelid, k.ord::int, true), \
                                       chr(31) ORDER BY k.ord) \
                       FROM unnest(i.indkey::int2[]) WITH ORDINALITY k(attnum, ord)), \
                    i.indisprimary \
             FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid \
             WHERE i.indrelid = $1::regclass ORDER BY ic.relname",
            std::slice::from_ref(&q),
        )?;
        let indexes = idx
            .iter()
            .map(|r| IndexInfo {
                name: text(&r[0]),
                columns: split_list(&r[2]),
                unique: r[1] == Value::Bool(true),
                primary: r[3] == Value::Bool(true),
            })
            .collect();
        // tgtype bits: 2 BEFORE, 64 INSTEAD OF (else AFTER); 4 INSERT, 8 DELETE,
        // 16 UPDATE, 32 TRUNCATE.
        let trg = self.rows(
            "SELECT t.tgname, \
                    CASE WHEN t.tgtype & 64 <> 0 THEN 'INSTEAD OF' \
                         WHEN t.tgtype & 2 <> 0 THEN 'BEFORE' ELSE 'AFTER' END, \
                    concat_ws(' OR ', \
                      CASE WHEN t.tgtype & 4 <> 0 THEN 'INSERT' END, \
                      CASE WHEN t.tgtype & 16 <> 0 THEN 'UPDATE' END, \
                      CASE WHEN t.tgtype & 8 <> 0 THEN 'DELETE' END, \
                      CASE WHEN t.tgtype & 32 <> 0 THEN 'TRUNCATE' END), \
                    'EXECUTE FUNCTION ' || t.tgfoid::regproc::text || '()', \
                    substring(pg_get_triggerdef(t.oid, true) from 'WHEN \\((.*)\\) EXECUTE') \
             FROM pg_trigger t \
             WHERE t.tgrelid = $1::regclass AND NOT t.tgisinternal ORDER BY t.tgname",
            std::slice::from_ref(&q),
        )?;
        let triggers = trg
            .iter()
            .map(|r| TriggerInfo {
                name: text(&r[0]),
                timing: text(&r[1]),
                event: text(&r[2]),
                action: text(&r[3]),
                condition: (!r[4].is_null()).then(|| text(&r[4])),
            })
            .collect();
        Ok(TableInfo {
            object: t.clone(),
            columns,
            foreign_keys,
            indexes,
            triggers,
        })
    }

    fn ddl(&mut self, o: &ObjectRef) -> Result<String> {
        let q = self.qualified(o);
        let quoted = text(&q);
        let kind = self
            .rows(
                "SELECT c.relkind::text FROM pg_class c WHERE c.oid = to_regclass($1)",
                std::slice::from_ref(&q),
            )?
            .first()
            .map(|r| text(&r[0]));
        match kind.as_deref() {
            Some("v") | Some("m") => {
                let def = self.rows(
                    "SELECT pg_get_viewdef($1::regclass, true)",
                    std::slice::from_ref(&q),
                )?;
                let m = if kind.as_deref() == Some("m") {
                    "MATERIALIZED "
                } else {
                    ""
                };
                Ok(format!(
                    "CREATE {m}VIEW {quoted} AS\n{}",
                    text(&def[0][0]).trim_end()
                ))
            }
            Some("i") => Ok(format!(
                "{};",
                text(
                    &self.rows(
                        "SELECT pg_get_indexdef($1::regclass)",
                        std::slice::from_ref(&q)
                    )?[0][0]
                )
            )),
            Some("S") => Ok(format!("CREATE SEQUENCE {quoted};")),
            Some(_) => self.table_ddl(o, &q),
            None => {
                // Not a relation: a routine or a trigger.
                let schema = Value::Text(o.schema.clone().unwrap_or_else(|| "public".into()));
                let name = Value::Text(o.name.clone());
                let f = self.rows(
                    "SELECT pg_get_functiondef(p.oid) FROM pg_proc p \
                     JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = $1 AND p.proname = $2 AND p.prokind IN ('f','p')",
                    &[schema.clone(), name.clone()],
                )?;
                if !f.is_empty() {
                    return Ok(f
                        .iter()
                        .map(|r| text(&r[0]))
                        .collect::<Vec<_>>()
                        .join(";\n\n"));
                }
                let t = self.rows(
                    "SELECT pg_get_triggerdef(tg.oid, true) FROM pg_trigger tg \
                     JOIN pg_class c ON c.oid = tg.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND tg.tgname = $2",
                    &[schema, name],
                )?;
                match t.first() {
                    Some(r) => Ok(format!("{};", text(&r[0]))),
                    None => Err(Error::Database(format!("no such object: {quoted}"))),
                }
            }
        }
    }
}

impl PgSession {
    fn table_ddl(&mut self, o: &ObjectRef, q: &Value) -> Result<String> {
        let info = self.describe_table(o)?;
        let d = Dialect::POSTGRES;
        let mut lines: Vec<String> = info
            .columns
            .iter()
            .map(|c| {
                let mut l = format!("    {} {}", d.quote_ident(&c.name), c.data_type);
                if !c.nullable {
                    l.push_str(" NOT NULL");
                }
                if let Some(def) = &c.default {
                    l.push_str(&format!(" DEFAULT {def}"));
                }
                l
            })
            .collect();
        let cons = self.rows(
            "SELECT conname, pg_get_constraintdef(oid, true) FROM pg_constraint \
             WHERE conrelid = $1::regclass AND contype IN ('p','u','f','c','x') \
             ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END, conname",
            std::slice::from_ref(q),
        )?;
        lines.extend(cons.iter().map(|r| {
            format!(
                "    CONSTRAINT {} {}",
                d.quote_ident(&text(&r[0])),
                text(&r[1])
            )
        }));
        let mut out = format!("CREATE TABLE {} (\n{}\n);", text(q), lines.join(",\n"));
        // Indexes that do not back a constraint, then triggers.
        let extra = self.rows(
            "SELECT pg_get_indexdef(i.indexrelid) FROM pg_index i \
             WHERE i.indrelid = $1::regclass \
               AND NOT EXISTS (SELECT 1 FROM pg_constraint c WHERE c.conindid = i.indexrelid) \
             UNION ALL \
             SELECT pg_get_triggerdef(tg.oid, true) FROM pg_trigger tg \
             WHERE tg.tgrelid = $1::regclass AND NOT tg.tgisinternal",
            std::slice::from_ref(q),
        )?;
        for r in extra {
            out.push_str(&format!("\n\n{};", text(&r[0])));
        }
        Ok(out)
    }
}

/// A foreign key's referential action from its `pg_constraint` code.
fn fk_action(code: &str) -> &'static str {
    match code {
        "r" => "RESTRICT",
        "c" => "CASCADE",
        "n" => "SET NULL",
        "d" => "SET DEFAULT",
        _ => "NO ACTION",
    }
}

#[cfg(test)]
mod tests {
    //! Live tests need a server: `THAAN_TEST_POSTGRES_URL=postgres://user:pw@host:port/db`.
    use super::*;
    use std::sync::Arc;
    use thaan_core::engine::{Engine, QueryRequest, RunScope};
    use thaan_core::store::Store;
    use thaan_core::table::{PageRequest, StagedChange};

    fn engine() -> Option<(Engine, u64)> {
        let url = std::env::var("THAAN_TEST_POSTGRES_URL").ok()?;
        let e = Engine::new(Store::in_memory());
        e.register_driver(Arc::new(PostgresDriver));
        let s = e.open(ConnectionConfig::from_url(&url).unwrap()).unwrap();
        Some((e, s.session_id))
    }

    fn run(e: &Engine, sid: u64, sql: &str) -> Vec<thaan_core::engine::StatementResult> {
        e.run_query(&QueryRequest {
            session_id: sid,
            sql: sql.into(),
            scope: RunScope::All,
            params: Default::default(),
        })
        .unwrap()
    }

    #[test]
    #[ignore = "needs THAAN_TEST_POSTGRES_URL"]
    fn live_roundtrip() {
        let Some((e, sid)) = engine() else { return };
        let r = run(
            &e,
            sid,
            "DROP SCHEMA IF EXISTS thaan_t CASCADE; CREATE SCHEMA thaan_t;
             CREATE TABLE thaan_t.users (id serial PRIMARY KEY, name text NOT NULL, score numeric(10,3),
               born date, seen timestamptz, tags text[], meta jsonb, uid uuid, ok boolean DEFAULT true);
             CREATE TABLE thaan_t.posts (id int PRIMARY KEY, user_id int REFERENCES thaan_t.users(id), title text);
             CREATE INDEX posts_user ON thaan_t.posts(user_id);
             CREATE VIEW thaan_t.v AS SELECT id, name FROM thaan_t.users;
             INSERT INTO thaan_t.users (name, score, born, seen, tags, meta, uid) VALUES
               ('ann', -1234.5, '1999-12-31', '2024-02-29 13:45:01.25+00', '{a,b}', '{\"k\": [1, 2]}',
                'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'),
               ('bob', 0.001, NULL, NULL, NULL, NULL, NULL);",
        );
        assert!(r.iter().all(|x| x.error.is_none()), "{r:?}");

        let r = run(
            &e,
            sid,
            "SELECT name, score, born, seen, tags, meta, uid, ok FROM thaan_t.users ORDER BY id",
        );
        let rs = r[0].result.as_ref().unwrap();
        assert_eq!(
            rs.rows[0],
            vec![
                Value::from("ann"),
                Value::from("-1234.500"),
                Value::from("1999-12-31"),
                Value::from("2024-02-29 13:45:01.25+00"),
                Value::from("{a,b}"),
                Value::from("{\"k\": [1, 2]}"),
                Value::from("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"),
                Value::Bool(true),
            ]
        );
        assert_eq!(rs.rows[1][1], Value::from("0.001"));

        // Bound parameters of several types are cast by the server.
        let mut params = std::collections::BTreeMap::new();
        params.insert("n".to_string(), Value::from("ann"));
        params.insert("min".to_string(), Value::Int(-2000));
        let r = e
            .run_query(&QueryRequest {
                session_id: sid,
                sql: "SELECT id FROM thaan_t.users WHERE name = :n AND score > :min".into(),
                scope: RunScope::All,
                params,
            })
            .unwrap();
        assert_eq!(
            r[0].result.as_ref().unwrap().rows,
            vec![vec![Value::Int(1)]]
        );

        let objs = e.list_objects(sid).unwrap();
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::View && o.object.name == "v"));
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::Index && o.object.name == "posts_user"));
        let t = ObjectRef {
            schema: Some("thaan_t".into()),
            name: "users".into(),
        };
        let info = e.describe_table(sid, &t).unwrap();
        assert_eq!(info.primary_key(), ["id"]);
        assert!(!info.columns[1].nullable);
        let posts = e
            .describe_table(
                sid,
                &ObjectRef {
                    schema: Some("thaan_t".into()),
                    name: "posts".into(),
                },
            )
            .unwrap();
        assert_eq!(posts.foreign_keys[0].columns, ["user_id"]);
        assert!(e
            .ddl(sid, &t)
            .unwrap()
            .starts_with("CREATE TABLE \"thaan_t\".\"users\""));

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
        run(&e, sid, "DROP SCHEMA thaan_t CASCADE");
    }
}
