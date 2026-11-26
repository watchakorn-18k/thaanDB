//! The sample database: a small shop in an in-memory SQLite, built from SQL at start-up so
//! the page ships no data file. Names and numbers are plausible, made up, and the same on
//! every load (a fixed pseudo-random sequence).

use std::sync::Arc;
use thaan_core::connection::ConnectionConfig;
use thaan_core::engine::{Engine, QueryRequest, RunScope, SessionInfo};
use thaan_core::store::Store;

pub const TABLES: [&str; 3] = ["customers", "products", "orders"];

const SCHEMA: &str = r#"
create table customers (
  id integer primary key,
  name text not null,
  email text not null unique,
  city text not null,
  joined_at text not null
);
create table products (
  id integer primary key,
  name text not null,
  category text not null,
  price real not null,
  stock integer not null
);
create table orders (
  id integer primary key,
  customer_id integer not null references customers(id),
  product_id integer not null references products(id),
  quantity integer not null,
  total real not null,
  status text not null,
  ordered_at text not null
);
create index orders_customer on orders(customer_id);
"#;

const FIRST: [&str; 16] = [
    "Ann", "Somchai", "Mali", "Niran", "Kanya", "Pim", "Arthit", "Suda", "Tom", "Lina", "Krit",
    "Ploy", "Wit", "Nok", "Ben", "Fah",
];
const LAST: [&str; 12] = [
    "Srisuk", "Wongsa", "Chaiyo", "Rattana", "Boonmee", "Saetang", "Lee", "Park", "Tan",
    "Kaewmanee", "Suwan", "Thong",
];
const CITIES: [&str; 8] = [
    "Bangkok", "Chiang Mai", "Khon Kaen", "Phuket", "Hat Yai", "Udon Thani", "Nakhon Ratchasima",
    "Pattaya",
];
const PRODUCTS: [(&str, &str, f64); 20] = [
    ("Espresso beans 1 kg", "Coffee", 690.0),
    ("Drip kettle", "Coffee", 1290.0),
    ("Hand grinder", "Coffee", 2450.0),
    ("Paper filters ×100", "Coffee", 149.0),
    ("Oolong tea 200 g", "Tea", 380.0),
    ("Jasmine tea 200 g", "Tea", 320.0),
    ("Glass teapot", "Tea", 590.0),
    ("Ceramic mug", "Kitchen", 250.0),
    ("Cast iron pan", "Kitchen", 1890.0),
    ("Chef knife", "Kitchen", 2190.0),
    ("Cutting board", "Kitchen", 450.0),
    ("Rice cooker", "Appliances", 3290.0),
    ("Electric kettle", "Appliances", 990.0),
    ("Toaster", "Appliances", 1490.0),
    ("Linen apron", "Home", 690.0),
    ("Cotton towels ×2", "Home", 390.0),
    ("Desk lamp", "Home", 1190.0),
    ("Notebook A5", "Stationery", 120.0),
    ("Fountain pen", "Stationery", 890.0),
    ("Ink bottle", "Stationery", 260.0),
];
const STATUS: [&str; 4] = ["paid", "shipped", "delivered", "cancelled"];

/// A small linear congruential generator: the same data on every load.
struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The INSERTs for the sample rows.
fn data() -> String {
    let mut r = Rng(42);
    let mut sql = String::new();
    for id in 1..=60 {
        let (f, l) = (FIRST[r.next(FIRST.len())], LAST[r.next(LAST.len())]);
        sql.push_str(&format!(
            "insert into customers values ({id}, {}, {}, {}, '2025-{:02}-{:02}');\n",
            q(&format!("{f} {l}")),
            q(&format!("{}.{}{id}@example.com", f.to_lowercase(), l.to_lowercase())),
            q(CITIES[r.next(CITIES.len())]),
            1 + r.next(12),
            1 + r.next(28),
        ));
    }
    for (i, (name, cat, price)) in PRODUCTS.iter().enumerate() {
        sql.push_str(&format!(
            "insert into products values ({}, {}, {}, {price}, {});\n",
            i + 1,
            q(name),
            q(cat),
            r.next(120),
        ));
    }
    for id in 1..=480 {
        let p = r.next(PRODUCTS.len());
        let qty = 1 + r.next(4);
        let status = STATUS[match r.next(20) {
            0 => 3,
            1..=6 => 0,
            7..=12 => 1,
            _ => 2,
        }];
        sql.push_str(&format!(
            "insert into orders values ({id}, {}, {}, {qty}, {}, '{status}', '2026-{:02}-{:02} {:02}:{:02}');\n",
            1 + r.next(60),
            p + 1,
            PRODUCTS[p].2 * qty as f64,
            1 + r.next(11),
            1 + r.next(28),
            8 + r.next(12),
            r.next(60),
        ));
    }
    sql
}

/// The engine with the SQLite driver, and the open sample database.
pub fn open() -> thaan_core::Result<(Arc<Engine>, SessionInfo)> {
    let engine = Arc::new(Engine::new(Store::in_memory()));
    engine.register_driver(Arc::new(thaan_driver_sqlite::SqliteDriver));
    let mut c = ConnectionConfig::from_url("sqlite::memory:")?;
    c.name = "Demo shop".into();
    let info = engine.open(c)?;
    let sql = format!("{SCHEMA}\n{}", data());
    for r in engine.run_query(&QueryRequest {
        session_id: info.session_id,
        sql,
        scope: RunScope::All,
        params: Default::default(),
    })? {
        if let Some(e) = r.error {
            return Err(thaan_core::Error::Database(e));
        }
    }
    // The setup statements are not the user's history.
    engine.clear_history().ok();
    Ok((engine, info))
}

pub const STARTER: &str = "-- Revenue by category, delivered orders only
select p.category,
       count(*)            as orders,
       sum(o.quantity)     as items,
       round(sum(o.total)) as revenue
from orders o
join products p on p.id = o.product_id
where o.status = 'delivered'
group by p.category
order by revenue desc;";
