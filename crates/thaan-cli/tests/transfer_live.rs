//! Data transfer between engines, against live servers. Ignored by default; each pair
//! runs only when its `THAAN_TEST_<NAME>_URL` is set (see AGENTS.md for throwaway
//! servers). A SQLite table with mixed types goes to the server and comes back into a
//! fresh SQLite database, and the row count and a few values must survive both ways.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use thaan_core::connection::ConnectionConfig;
use thaan_core::engine::{Engine, QueryRequest, RunScope, SessionId};
use thaan_core::schema::ObjectRef;
use thaan_core::store::Store;
use thaan_core::transfer::{ExistingTable, TransferContent, TransferRequest};
use thaan_core::Value;

const TABLE: &str = "thaan_transfer_people";
const ROWS: i64 = 1234;

fn engine() -> Engine {
    let e = Engine::new(Store::in_memory());
    e.register_driver(Arc::new(thaan_driver_sqlite::SqliteDriver));
    e.register_driver(Arc::new(thaan_driver_postgres::PostgresDriver));
    e.register_driver(Arc::new(thaan_driver_mysql::MysqlDriver));
    e.register_driver(Arc::new(thaan_driver_mongodb::MongoDriver));
    e
}

fn open(e: &Engine, url: &str) -> SessionId {
    e.open(ConnectionConfig::from_url(url).unwrap())
        .unwrap()
        .session_id
}

fn run(e: &Engine, sid: SessionId, sql: &str) -> Vec<Vec<Value>> {
    let r = e
        .run_query(&QueryRequest {
            session_id: sid,
            sql: sql.into(),
            scope: RunScope::All,
            params: Default::default(),
        })
        .unwrap();
    if let Some(err) = r.iter().find_map(|x| x.error.clone()) {
        panic!("{sql}: {err}");
    }
    r.into_iter()
        .last()
        .and_then(|x| x.result)
        .map(|rs| rs.rows)
        .unwrap_or_default()
}

fn transfer(e: &Engine, source: SessionId, target: SessionId) -> u64 {
    let req = TransferRequest {
        source,
        target,
        tables: vec![ObjectRef {
            schema: None,
            name: TABLE.into(),
        }],
        content: TransferContent::StructureAndData,
        existing: ExistingTable::Replace,
        batch_size: 500,
    };
    let r = e.transfer(&req, &|_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(r.tables, 1);
    r.rows
}

fn source(e: &Engine) -> SessionId {
    let s = open(e, "sqlite::memory:");
    run(
        e,
        s,
        &format!(
            "create table {TABLE} (id integer primary key, name text not null, age int, \
             score real, note text)"
        ),
    );
    let values: Vec<String> = (1..=ROWS)
        .map(|i| {
            let note = if i % 10 == 0 {
                "null".into()
            } else {
                format!("'n{i}'")
            };
            format!("({i}, 'user {i}', {}, {}.5, {note})", i % 90, i)
        })
        .collect();
    run(
        e,
        s,
        &format!("insert into {TABLE} values {}", values.join(",")),
    );
    s
}

/// SQLite → server → SQLite, checking what arrives back.
fn round_trip(name: &str) {
    let var = format!("THAAN_TEST_{name}_URL");
    let Ok(url) = std::env::var(&var) else {
        eprintln!("skip: {var} not set");
        return;
    };
    let e = engine();
    let src = source(&e);
    let server = open(&e, &url);
    assert_eq!(transfer(&e, src, server), ROWS as u64, "into {name}");
    let back = open(&e, "sqlite::memory:");
    assert_eq!(transfer(&e, server, back), ROWS as u64, "back from {name}");
    let count = run(&e, back, &format!("select count(*) from {TABLE}"));
    assert_eq!(count[0][0], Value::Int(ROWS));
    let row = run(
        &e,
        back,
        &format!("select name, age, score, note from {TABLE} where name = 'user 7'"),
    );
    assert_eq!(row.len(), 1, "{name}: user 7 came back");
    eprintln!("{name}: {:?}", row[0]);
    assert_eq!(row[0][0], Value::Text("user 7".into()));
    let nulls = run(
        &e,
        back,
        &format!("select count(*) from {TABLE} where note is null"),
    );
    assert_eq!(nulls[0][0], Value::Int(ROWS / 10), "{name}: nulls kept");
}

#[test]
#[ignore]
fn transfer_postgres() {
    round_trip("POSTGRES");
}

#[test]
#[ignore]
fn transfer_mysql() {
    round_trip("MYSQL");
}

#[test]
#[ignore]
fn transfer_mongodb() {
    round_trip("MONGODB");
}
