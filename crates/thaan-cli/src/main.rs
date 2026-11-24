//! `thaan` — headless client of the engine. Every GUI action is reachable from here via
//! `thaan run <commandId> '<json>'`.

use anyhow::{anyhow, bail, Context, Result};
use thaan_core::commands::{builtin_commands, dispatch};
use thaan_core::connection::ConnectionConfig;
use thaan_core::engine::{Engine, QueryRequest, RunScope};
use thaan_core::export::{ExportFormat, ExportOptions};
use thaan_core::schema::ObjectRef;
use thaan_core::store::Store;
use thaan_core::table::PageRequest;
use thaan_core::{ResultSet, Value};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::Arc;

const USAGE: &str = "\
thaan — ThaanDB command-line client

USAGE:
  thaan commands                            List every command id
  thaan drivers                             List drivers and their connection parameters
  thaan query <url> <sql> [opts]            Run SQL (all statements)
        --format table|csv|tsv|json|jsonl|sql|md   (default: table)
        --param name=value                  Value for :name / $1 / ? (repeatable; $1 → 1=…)
        --read-only                         Refuse writes
  thaan tables <url>                        List tables, views, indexes, triggers
  thaan describe <url> <table>              Columns, keys and indexes as JSON
  thaan ddl <url> <object>                  CREATE statement(s)
  thaan export <url> <table> [--format csv] [--out FILE] [--where SQL]
  thaan run <commandId> [json] [--url URL]  Dispatch any command; with --url a session is
                                            opened first and `sessionId` is filled in
  Global: --store FILE   use this store instead of the default (connections, history)

URL examples: sqlite:./app.db  sqlite::memory:  postgres://user@host/db
              mysql://root@localhost/app  mongodb://localhost:27017/shop
MongoDB queries use shell syntax: db.users.find({ age: { $gt: 30 } }).limit(10)
";

struct Args {
    positional: Vec<String>,
    flags: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn parse() -> Args {
        let mut positional = Vec::new();
        let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                let takes_value = !matches!(name, "read-only" | "help");
                let v = if takes_value {
                    it.next().unwrap_or_default()
                } else {
                    String::new()
                };
                flags.entry(name.to_string()).or_default().push(v);
            } else {
                positional.push(a);
            }
        }
        Args { positional, flags }
    }
    fn flag(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.flags.contains_key(k)
    }
    fn pos(&self, i: usize, what: &str) -> Result<&str> {
        self.positional
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| anyhow!("missing <{what}>\n\n{USAGE}"))
    }
}

fn engine(args: &Args) -> Result<Engine> {
    let store = match args
        .flag("store")
        .map(Into::into)
        .or_else(Store::default_path)
    {
        Some(p) => Store::open(p).context("opening store")?,
        None => Store::in_memory(),
    };
    let e = Engine::new(store);
    #[cfg(feature = "sqlite")]
    e.register_driver(Arc::new(thaan_driver_sqlite::SqliteDriver));
    #[cfg(feature = "postgres")]
    e.register_driver(Arc::new(thaan_driver_postgres::PostgresDriver));
    #[cfg(feature = "mysql")]
    e.register_driver(Arc::new(thaan_driver_mysql::MysqlDriver));
    #[cfg(feature = "mongodb")]
    e.register_driver(Arc::new(thaan_driver_mongodb::MongoDriver));
    Ok(e)
}

fn open(e: &Engine, url: &str, read_only: bool) -> Result<u64> {
    // A saved connection id or name also works in place of a URL.
    let saved = e
        .connections()
        .into_iter()
        .find(|c| c.id == url || c.name == url);
    let mut c = match saved {
        Some(c) => c,
        None => ConnectionConfig::from_url(url)?,
    };
    c.read_only |= read_only;
    Ok(e.open(c)?.session_id)
}

fn parse_format(s: &str) -> Result<Option<ExportFormat>> {
    Ok(Some(match s {
        "table" => return Ok(None),
        "csv" => ExportFormat::Csv,
        "tsv" => ExportFormat::Tsv,
        "json" => ExportFormat::Json,
        "jsonl" => ExportFormat::JsonLines,
        "sql" => ExportFormat::SqlInsert,
        "md" | "markdown" => ExportFormat::Markdown,
        other => bail!("unknown format `{other}`"),
    }))
}

fn print_table(rs: &ResultSet) {
    if rs.columns.is_empty() {
        println!(
            "OK, {} row(s) affected ({} ms)",
            rs.rows_affected.unwrap_or(0),
            rs.elapsed_ms
        );
        return;
    }
    const MAX: usize = 60;
    let cell = |v: &Value| {
        let s = v.to_string().replace('\n', "⏎");
        if s.chars().count() > MAX {
            format!("{}…", s.chars().take(MAX - 1).collect::<String>())
        } else {
            s
        }
    };
    let rows: Vec<Vec<String>> = rs
        .rows
        .iter()
        .map(|r| r.iter().map(cell).collect())
        .collect();
    let mut w: Vec<usize> = rs.columns.iter().map(|c| c.name.chars().count()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c}{}", " ".repeat(w[i] - c.chars().count())))
            .collect();
        println!("│ {} │", parts.join(" │ "));
    };
    let sep = |l: &str, m: &str, r: &str| {
        let parts: Vec<String> = w.iter().map(|n| "─".repeat(n + 2)).collect();
        println!("{l}{}{r}", parts.join(m));
    };
    sep("┌", "┬", "┐");
    line(rs.columns.iter().map(|c| c.name.as_str()).collect());
    sep("├", "┼", "┤");
    for r in &rows {
        line(r.iter().map(String::as_str).collect());
    }
    sep("└", "┴", "┘");
    println!("{} row(s) ({} ms)", rs.rows.len(), rs.elapsed_ms);
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args = Args::parse();
    let Some(cmd) = args.positional.first().map(String::as_str) else {
        print!("{USAGE}");
        return Ok(());
    };
    if args.has("help") {
        print!("{USAGE}");
        return Ok(());
    }
    let e = engine(&args)?;
    match cmd {
        "commands" => {
            for c in builtin_commands() {
                let short = c.shortcut.map(|s| format!("  [{s}]")).unwrap_or_default();
                println!("{:<24} {:<28} {}{}", c.id, c.title, c.params, short);
            }
        }
        "drivers" => println!("{}", serde_json::to_string_pretty(&e.drivers())?),
        "query" => {
            let sid = open(&e, args.pos(1, "url")?, args.has("read-only"))?;
            let sql = args.pos(2, "sql")?.to_string();
            let mut params = BTreeMap::new();
            for p in args.flags.get("param").into_iter().flatten() {
                let (k, v) = p
                    .split_once('=')
                    .ok_or_else(|| anyhow!("--param expects name=value"))?;
                let key = if k.chars().all(|c| c.is_ascii_digit()) {
                    // `$1=…` and `?1=…` both work as `1=…`
                    if sql.contains('$') {
                        format!("${k}")
                    } else {
                        format!("?{k}")
                    }
                } else {
                    k.trim_start_matches([':', '$']).to_string()
                };
                let val = serde_json::from_str::<Json>(v)
                    .map(|j| Value::from_json(&j))
                    .unwrap_or(Value::from(v));
                params.insert(key, val);
            }
            let fmt = parse_format(args.flag("format").unwrap_or("table"))?;
            let results = e.run_query(&QueryRequest {
                session_id: sid,
                sql,
                scope: RunScope::All,
                params,
            })?;
            let mut failed = false;
            for r in results {
                match (r.result, r.error) {
                    (_, Some(err)) => {
                        eprintln!("error in `{}`: {err}", r.sql);
                        failed = true;
                    }
                    (Some(rs), None) => match fmt {
                        None => print_table(&rs),
                        Some(f) => print!("{}", e.export_result(sid, &rs, &ExportOptions::new(f))?),
                    },
                    _ => {}
                }
            }
            if failed {
                std::process::exit(2);
            }
        }
        "tables" => {
            let sid = open(&e, args.pos(1, "url")?, true)?;
            for o in e.list_objects(sid)? {
                let parent = o.parent.map(|p| format!("  (on {p})")).unwrap_or_default();
                println!(
                    "{:<8} {}{}",
                    format!("{:?}", o.kind).to_lowercase(),
                    o.object.name,
                    parent
                );
            }
        }
        "describe" => {
            let sid = open(&e, args.pos(1, "url")?, true)?;
            let t = e.describe_table(sid, &ObjectRef::new(args.pos(2, "table")?))?;
            println!("{}", serde_json::to_string_pretty(&t)?);
        }
        "ddl" => {
            let sid = open(&e, args.pos(1, "url")?, true)?;
            println!("{}", e.ddl(sid, &ObjectRef::new(args.pos(2, "object")?))?);
        }
        "export" => {
            let sid = open(&e, args.pos(1, "url")?, true)?;
            let fmt =
                parse_format(args.flag("format").unwrap_or("csv"))?.unwrap_or(ExportFormat::Csv);
            let req = PageRequest {
                table: ObjectRef::new(args.pos(2, "table")?),
                filters: vec![],
                raw_where: args.flag("where").map(str::to_string),
                sort: vec![],
                page: 0,
                page_size: 100,
            };
            let text = e.export_table(sid, &req, &ExportOptions::new(fmt))?;
            match args.flag("out") {
                Some(p) => {
                    std::fs::write(p, &text)?;
                    eprintln!("wrote {} bytes to {p}", text.len());
                }
                None => print!("{text}"),
            }
        }
        "run" => {
            let id = args.pos(1, "commandId")?;
            let mut params: Json = match args.positional.get(2) {
                Some(s) => serde_json::from_str(s).context("params must be JSON")?,
                None => json!({}),
            };
            if let Some(url) = args.flag("url") {
                let sid = open(&e, url, args.has("read-only"))?;
                if params.get("sessionId").is_none() {
                    params["sessionId"] = json!(sid);
                }
            }
            let out = dispatch(&e, id, &params)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        other => bail!("unknown subcommand `{other}`\n\n{USAGE}"),
    }
    Ok(())
}
