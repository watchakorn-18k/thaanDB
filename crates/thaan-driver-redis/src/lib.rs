//! Redis driver. Written from the Redis docs (RESP2 protocol spec and the command
//! reference at <https://redis.io/docs/latest/commands/>). Pure Rust: a small RESP client
//! over `std::net::TcpStream`, no dependencies beyond `thaan-core`.
//!
//! Model:
//! - Databases are the numbered keyspaces `db0`, `db1`, … (`SELECT n`).
//! - Every key is an object in the tree (the first [`KEY_LIMIT`] keys `SCAN` returns).
//! - Opening a key shows its contents as rows, by type: a string is one `value` row, a hash
//!   is `field | value`, a list `index | value`, a set `member`, a sorted set
//!   `member | score`, a stream `id | fields`. Pages come from `LRANGE`, `ZRANGE` and the
//!   `*SCAN` family, so a big key is never read whole.
//! - The query editor takes one Redis command per line (`HGETALL user:1`).
//! - Staged edits become `HSET` / `HDEL` / `LSET` / `SADD` / `ZADD` … run in one
//!   `MULTI` / `EXEC` block.

pub mod resp;

use resp::{read_reply, write_command, Reply};
use std::io::BufReader;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use thaan_core::browse::{KeyEntry, KeyPage, KeyScan};
use thaan_core::cmdline::{quote_arg, split_args};
use thaan_core::connection::ConnectionConfig;
use thaan_core::dialect::Dialect;
use thaan_core::driver::{Driver, DriverInfo, ParamKind, ParamSpec, Session};
use thaan_core::schema::{ColumnInfo, ObjectKind, ObjectRef, SchemaObject, TableInfo};
use thaan_core::table::{Filter, FilterOp, PageRequest, StagedChange};
use thaan_core::{Column, Error, Result, ResultSet, Value};

/// Keys listed in the object tree. Bigger keyspaces show the first ones `SCAN` returns;
/// the query editor (`SCAN 0 MATCH user:* COUNT 100`) reaches the rest.
pub const KEY_LIMIT: usize = 1000;
/// Rows a query result keeps (`KEYS *` on a big database).
pub const ROW_CAP: usize = 10_000;
/// `COUNT` hint for the `*SCAN` commands.
const SCAN_COUNT: &[u8] = b"500";

pub struct RedisDriver;

impl Driver for RedisDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "redis".into(),
            name: "Redis".into(),
            params: vec![
                ParamSpec::new("host", "Host", ParamKind::Text).default("localhost"),
                ParamSpec::new("port", "Port", ParamKind::Number).default("6379"),
                ParamSpec::new("user", "User", ParamKind::Text).placeholder("(optional, ACL user)"),
                ParamSpec::new("password", "Password", ParamKind::Password),
                ParamSpec::new("database", "Database", ParamKind::Text).placeholder("0"),
            ],
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::REDIS
    }

    fn connect(&self, c: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let host = c.param("host").unwrap_or("localhost");
        let port: u16 = c
            .param("port")
            .unwrap_or("6379")
            .parse()
            .map_err(|_| Error::InvalidParams("port must be a number".into()))?;
        let db = parse_db(c.param("database").unwrap_or("0"))?;
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(|e| Error::Database(format!("{host}: {e}")))?
            .next()
            .ok_or_else(|| Error::Database(format!("{host}: no address")))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(io)?;
        stream.set_nodelay(true).ok();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(io)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(io)?;
        let writer = stream.try_clone().map_err(io)?;
        let mut s = RedisSession {
            reader: BufReader::new(stream),
            writer,
            db,
            read_only: c.read_only,
        };
        if let Some(pw) = c.param("password") {
            match c.param("user") {
                Some(u) => s.cmd(&[b"AUTH", u.as_bytes(), pw.as_bytes()])?,
                None => s.cmd(&[b"AUTH", pw.as_bytes()])?,
            };
        }
        s.cmd(&[b"CLIENT", b"SETNAME", b"thaan"]).ok();
        if db != 0 {
            s.cmd(&[b"SELECT", db.to_string().as_bytes()])?;
        }
        Ok(Box::new(s))
    }
}

/// `"3"`, `"db3"` → 3.
fn parse_db(s: &str) -> Result<u32> {
    let t = s.trim();
    let t = t.strip_prefix("db").unwrap_or(t);
    if t.is_empty() {
        return Ok(0);
    }
    t.parse()
        .map_err(|_| Error::InvalidParams(format!("database must be a number, got `{s}`")))
}

fn io(e: std::io::Error) -> Error {
    Error::Database(e.to_string())
}

pub struct RedisSession {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    db: u32,
    read_only: bool,
}

impl RedisSession {
    /// Sends one command; a Redis error reply becomes `Error::Database`.
    fn cmd(&mut self, args: &[&[u8]]) -> Result<Reply> {
        match self.raw(args)? {
            Reply::Error(e) => Err(Error::Database(e)),
            r => Ok(r),
        }
    }

    fn raw(&mut self, args: &[&[u8]]) -> Result<Reply> {
        write_command(&mut self.writer, args).map_err(io)?;
        read_reply(&mut self.reader).map_err(io)
    }

    /// Sends every command, then reads every reply: one round trip for the batch.
    /// Error replies are returned in place, not raised.
    fn pipeline(&mut self, cmds: &[Vec<&[u8]>]) -> Result<Vec<Reply>> {
        let mut buf = Vec::new();
        for c in cmds {
            write_command(&mut buf, c).map_err(io)?;
        }
        std::io::Write::write_all(&mut self.writer, &buf).map_err(io)?;
        cmds.iter()
            .map(|_| read_reply(&mut self.reader).map_err(io))
            .collect()
    }

    /// One `SCAN` step with the type and TTL of each key it returned.
    fn scan_step(&mut self, scan: &KeyScan) -> Result<KeyPage> {
        let cursor = if scan.cursor.is_empty() {
            "0"
        } else {
            scan.cursor.as_str()
        };
        let count = scan.count.clamp(10, 5000).to_string();
        let mut args: Vec<&[u8]> = vec![b"SCAN", cursor.as_bytes()];
        if let Some(p) = scan
            .pattern
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            args.extend([b"MATCH".as_slice(), p.as_bytes()]);
        }
        args.extend([b"COUNT".as_slice(), count.as_bytes()]);
        if let Some(t) = scan.key_type.as_deref().filter(|t| !t.is_empty()) {
            args.extend([b"TYPE".as_slice(), t.as_bytes()]);
        }
        let mut parts = self.cmd(&args)?.array().into_iter();
        let next = parts
            .next()
            .and_then(|r| r.text())
            .unwrap_or_else(|| "0".into());
        let names: Vec<Vec<u8>> = parts
            .next()
            .map(Reply::array)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.bytes())
            .collect();
        let mut cmds: Vec<Vec<&[u8]>> = Vec::with_capacity(names.len() * 2 + 1);
        for n in &names {
            cmds.push(vec![b"TYPE", n]);
            cmds.push(vec![b"TTL", n]);
        }
        cmds.push(vec![b"DBSIZE"]);
        let mut replies = self.pipeline(&cmds)?.into_iter();
        let mut keys = Vec::with_capacity(names.len());
        for n in names {
            let ty = replies.next().and_then(|r| r.text()).unwrap_or_default();
            let ttl = replies.next().and_then(|r| r.int()).unwrap_or(-1);
            if ty == "none" {
                continue; // expired between SCAN and TYPE
            }
            keys.push(KeyEntry {
                name: String::from_utf8_lossy(&n).into_owned(),
                key_type: ty,
                ttl: (ttl >= 0).then_some(ttl),
            });
        }
        let total = replies
            .next()
            .and_then(|r| r.int())
            .map(|n| n.max(0) as u64);
        Ok(KeyPage {
            keys,
            cursor: next,
            total,
        })
    }

    fn cmd_owned(&mut self, args: &[Vec<u8>]) -> Result<Reply> {
        let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
        self.cmd(&refs)
    }

    fn key_type(&mut self, key: &str) -> Result<String> {
        Ok(self
            .cmd(&[b"TYPE", key.as_bytes()])?
            .text()
            .unwrap_or_default())
    }

    /// Runs a `*SCAN` command from cursor 0, skipping `offset` items and keeping `limit`
    /// (each item is `per` replies: 1 for SSCAN, 2 for HSCAN / ZSCAN). Returns the kept
    /// items and the number of matches (counted to the end only when `count_all`).
    fn scan_page(
        &mut self,
        command: &[u8],
        key: &str,
        pattern: Option<&str>,
        per: usize,
        (offset, limit): (u64, u64),
        count_all: bool,
    ) -> Result<(Vec<Vec<Reply>>, u64)> {
        let mut cursor = b"0".to_vec();
        let mut seen = 0u64;
        let mut out = Vec::new();
        loop {
            let mut args: Vec<&[u8]> = vec![command, key.as_bytes(), &cursor];
            if let Some(p) = pattern {
                args.extend([b"MATCH".as_slice(), p.as_bytes()]);
            }
            args.extend([b"COUNT".as_slice(), SCAN_COUNT]);
            let mut parts = self.cmd(&args)?.array().into_iter();
            let next = parts
                .next()
                .and_then(|r| r.bytes())
                .unwrap_or(b"0".to_vec());
            let items = parts.next().map(Reply::array).unwrap_or_default();
            let mut it = items.into_iter();
            loop {
                let item: Vec<Reply> = it.by_ref().take(per).collect();
                if item.len() < per {
                    break;
                }
                if seen >= offset && (out.len() as u64) < limit {
                    out.push(item);
                }
                seen += 1;
            }
            cursor = next;
            let full = out.len() as u64 >= limit;
            if cursor == b"0" || (full && !count_all) {
                break;
            }
        }
        Ok((out, seen))
    }

    /// The commands that apply `changes` to `key`, in order (without MULTI / EXEC).
    fn plan(&mut self, key: &str, changes: &[StagedChange]) -> Result<Vec<Vec<Vec<u8>>>> {
        let ty = self.key_type(key)?;
        let k = key.as_bytes().to_vec();
        let c = |parts: &[&[u8]]| parts.iter().map(|p| p.to_vec()).collect::<Vec<_>>();
        let mut out: Vec<Vec<Vec<u8>>> = Vec::new();
        let mut tail: Vec<Vec<Vec<u8>>> = Vec::new();
        let tomb = b"\x00thaan-deleted\x00".to_vec();
        let mut list_deletes = false;
        for ch in changes {
            match (ty.as_str(), ch) {
                ("string", StagedChange::Update { set, .. }) => {
                    let v = field(set, "value").ok_or_else(|| missing("value"))?;
                    out.push(c(&[b"SET", &k, &v, b"KEEPTTL"]));
                }
                ("string", StagedChange::Delete { .. }) => out.push(c(&[b"DEL", &k])),
                ("hash", StagedChange::Insert { values }) => {
                    let f = field(values, "field").ok_or_else(|| missing("field"))?;
                    let v = field(values, "value").unwrap_or_default();
                    out.push(c(&[b"HSET", &k, &f, &v]));
                }
                ("hash", StagedChange::Update { key: row, set }) => {
                    let old = field(row, "field").ok_or_else(|| missing("field"))?;
                    let new = field(set, "field").unwrap_or_else(|| old.clone());
                    let v = match field(set, "value") {
                        Some(v) => v,
                        None => self.cmd(&[b"HGET", &k, &old])?.bytes().unwrap_or_default(),
                    };
                    if new != old {
                        out.push(c(&[b"HDEL", &k, &old]));
                    }
                    out.push(c(&[b"HSET", &k, &new, &v]));
                }
                ("hash", StagedChange::Delete { key: row }) => {
                    let f = field(row, "field").ok_or_else(|| missing("field"))?;
                    out.push(c(&[b"HDEL", &k, &f]));
                }
                ("list", StagedChange::Insert { values }) => {
                    let v = field(values, "value").unwrap_or_default();
                    tail.push(c(&[b"RPUSH", &k, &v]));
                }
                ("list", StagedChange::Update { key: row, set }) => {
                    let i = field(row, "index").ok_or_else(|| missing("index"))?;
                    let v = field(set, "value").ok_or_else(|| missing("value"))?;
                    out.push(c(&[b"LSET", &k, &i, &v]));
                }
                ("list", StagedChange::Delete { key: row }) => {
                    // Mark by index first, then remove every mark at once, so earlier
                    // deletes do not shift the indexes of later ones.
                    let i = field(row, "index").ok_or_else(|| missing("index"))?;
                    out.push(c(&[b"LSET", &k, &i, &tomb]));
                    list_deletes = true;
                }
                ("set", StagedChange::Insert { values }) => {
                    let m = field(values, "member").ok_or_else(|| missing("member"))?;
                    out.push(c(&[b"SADD", &k, &m]));
                }
                ("set", StagedChange::Update { key: row, set }) => {
                    let old = field(row, "member").ok_or_else(|| missing("member"))?;
                    let new = field(set, "member").ok_or_else(|| missing("member"))?;
                    out.push(c(&[b"SREM", &k, &old]));
                    out.push(c(&[b"SADD", &k, &new]));
                }
                ("set", StagedChange::Delete { key: row }) => {
                    let m = field(row, "member").ok_or_else(|| missing("member"))?;
                    out.push(c(&[b"SREM", &k, &m]));
                }
                ("zset", StagedChange::Insert { values }) => {
                    let m = field(values, "member").ok_or_else(|| missing("member"))?;
                    let s = field(values, "score").unwrap_or(b"0".to_vec());
                    out.push(c(&[b"ZADD", &k, &s, &m]));
                }
                ("zset", StagedChange::Update { key: row, set }) => {
                    let old = field(row, "member").ok_or_else(|| missing("member"))?;
                    let new = field(set, "member").unwrap_or_else(|| old.clone());
                    let s = match field(set, "score") {
                        Some(s) => s,
                        None => self
                            .cmd(&[b"ZSCORE", &k, &old])?
                            .bytes()
                            .unwrap_or(b"0".to_vec()),
                    };
                    if new != old {
                        out.push(c(&[b"ZREM", &k, &old]));
                    }
                    out.push(c(&[b"ZADD", &k, &s, &new]));
                }
                ("zset", StagedChange::Delete { key: row }) => {
                    let m = field(row, "member").ok_or_else(|| missing("member"))?;
                    out.push(c(&[b"ZREM", &k, &m]));
                }
                ("stream", StagedChange::Insert { values }) => {
                    let text = field(values, "fields").unwrap_or(b"{}".to_vec());
                    let obj: serde_json::Map<String, serde_json::Value> =
                        serde_json::from_slice(&text).map_err(|e| {
                            Error::InvalidParams(format!(
                                "fields must be a JSON object like {{\"a\": \"1\"}}: {e}"
                            ))
                        })?;
                    if obj.is_empty() {
                        return Err(Error::InvalidParams("a stream entry needs fields".into()));
                    }
                    let id = field(values, "id").unwrap_or(b"*".to_vec());
                    let mut cmd = c(&[b"XADD", &k, &id]);
                    for (f, v) in obj {
                        cmd.push(f.into_bytes());
                        cmd.push(match v {
                            serde_json::Value::String(s) => s.into_bytes(),
                            other => other.to_string().into_bytes(),
                        });
                    }
                    out.push(cmd);
                }
                ("stream", StagedChange::Delete { key: row }) => {
                    let id = field(row, "id").ok_or_else(|| missing("id"))?;
                    out.push(c(&[b"XDEL", &k, &id]));
                }
                (t, ch) => {
                    let what = match ch {
                        StagedChange::Insert { .. } => "adding rows",
                        StagedChange::Update { .. } => "editing rows",
                        StagedChange::Delete { .. } => "deleting rows",
                    };
                    return Err(Error::Unsupported(format!("{what} in a {t} key")));
                }
            }
        }
        if list_deletes {
            out.push(c(&[b"LREM", &k, b"0", &tomb]));
        }
        out.extend(tail);
        Ok(out)
    }
}

fn missing(col: &str) -> Error {
    Error::InvalidParams(format!("missing `{col}`"))
}

/// The value of column `name` as bytes.
fn field(cols: &[(String, Value)], name: &str) -> Option<Vec<u8>> {
    cols.iter().find(|(c, _)| c == name).map(|(_, v)| match v {
        Value::Null => Vec::new(),
        Value::Bool(b) => {
            if *b {
                b"1".to_vec()
            } else {
                b"0".to_vec()
            }
        }
        Value::Int(i) => i.to_string().into_bytes(),
        Value::Float(f) => f.to_string().into_bytes(),
        Value::Text(s) => s.clone().into_bytes(),
        Value::Bytes(b) => b.clone(),
    })
}

fn val(b: Vec<u8>) -> Value {
    match String::from_utf8(b) {
        Ok(s) => Value::Text(s),
        Err(e) => Value::Bytes(e.into_bytes()),
    }
}

fn reply_val(r: Reply) -> Value {
    match r {
        Reply::Nil => Value::Null,
        Reply::Int(i) => Value::Int(i),
        Reply::Status(s) => Value::Text(s),
        Reply::Error(e) => Value::Text(format!("(error) {e}")),
        Reply::Bulk(b) => val(b),
        Reply::Array(a) => Value::Text(render(&Reply::Array(a))),
    }
}

/// A nested reply as compact JSON-like text, for one grid cell.
fn render(r: &Reply) -> String {
    match r {
        Reply::Nil => "null".into(),
        Reply::Int(i) => i.to_string(),
        Reply::Status(s) => s.clone(),
        Reply::Error(e) => format!("(error) {e}"),
        Reply::Bulk(b) => {
            serde_json::Value::String(String::from_utf8_lossy(b).into_owned()).to_string()
        }
        Reply::Array(a) => format!("[{}]", a.iter().map(render).collect::<Vec<_>>().join(", ")),
    }
}

fn cols(names: &[&str]) -> Vec<Column> {
    names.iter().map(|n| Column::new(*n)).collect()
}

/// Stream entries (`[id, [f, v, …]]`) as `id | fields` rows, fields as a JSON object.
fn stream_rows(entries: Vec<Reply>) -> Vec<Vec<Value>> {
    entries
        .into_iter()
        .map(|e| {
            let mut parts = e.array().into_iter();
            let id = parts.next().map(reply_val).unwrap_or_default();
            let mut obj = serde_json::Map::new();
            let mut fv = parts
                .next()
                .map(Reply::array)
                .unwrap_or_default()
                .into_iter();
            while let (Some(f), Some(v)) = (fv.next(), fv.next()) {
                obj.insert(
                    f.text().unwrap_or_default(),
                    serde_json::Value::String(v.text().unwrap_or_default()),
                );
            }
            vec![id, Value::Text(serde_json::Value::Object(obj).to_string())]
        })
        .collect()
}

/// `MATCH` glob for the key view's filters: `like` maps `%`/`_` to `*`/`?`, `=` matches
/// the literal text, and the raw box takes a glob as typed.
fn match_pattern(filters: &[Filter], raw: Option<&str>, column: &str) -> Result<Option<String>> {
    let escape = |s: &str| {
        s.chars().fold(String::new(), |mut o, c| {
            if matches!(c, '*' | '?' | '[' | ']' | '\\') {
                o.push('\\');
            }
            o.push(c);
            o
        })
    };
    let mut pat = None;
    for f in filters {
        let text = match f.values.first() {
            Some(Value::Text(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        };
        let p = match (f.op, f.column == column) {
            (FilterOp::Eq, true) => escape(&text),
            (FilterOp::Like, true) => text.chars().fold(String::new(), |mut o, c| {
                match c {
                    '%' => o.push('*'),
                    '_' => o.push('?'),
                    '*' | '?' | '[' | ']' | '\\' => {
                        o.push('\\');
                        o.push(c);
                    }
                    c => o.push(c),
                }
                o
            }),
            _ => {
                return Err(Error::Unsupported(format!(
                    "Redis keys filter by `{column}` with `=` or `like` only"
                )))
            }
        };
        if pat.replace(p).is_some() {
            return Err(Error::Unsupported(
                "one filter at a time on Redis keys".into(),
            ));
        }
    }
    if let Some(r) = raw.map(str::trim).filter(|r| !r.is_empty()) {
        if pat.replace(r.to_string()).is_some() {
            return Err(Error::Unsupported(
                "one filter at a time on Redis keys".into(),
            ));
        }
    }
    Ok(pat)
}

/// Commands allowed on a read-only connection.
const READ_COMMANDS: &[&str] = &[
    "GET",
    "MGET",
    "STRLEN",
    "GETRANGE",
    "SUBSTR",
    "LCS",
    "EXISTS",
    "TYPE",
    "TTL",
    "PTTL",
    "EXPIRETIME",
    "PEXPIRETIME",
    "KEYS",
    "SCAN",
    "DBSIZE",
    "RANDOMKEY",
    "HGET",
    "HMGET",
    "HGETALL",
    "HKEYS",
    "HVALS",
    "HLEN",
    "HEXISTS",
    "HSTRLEN",
    "HSCAN",
    "HRANDFIELD",
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LPOS",
    "SMEMBERS",
    "SISMEMBER",
    "SMISMEMBER",
    "SCARD",
    "SRANDMEMBER",
    "SSCAN",
    "SINTER",
    "SUNION",
    "SDIFF",
    "SINTERCARD",
    "ZRANGE",
    "ZREVRANGE",
    "ZRANGEBYSCORE",
    "ZREVRANGEBYSCORE",
    "ZRANGEBYLEX",
    "ZREVRANGEBYLEX",
    "ZCARD",
    "ZSCORE",
    "ZMSCORE",
    "ZRANK",
    "ZREVRANK",
    "ZCOUNT",
    "ZLEXCOUNT",
    "ZSCAN",
    "ZRANDMEMBER",
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XINFO",
    "XPENDING",
    "INFO",
    "PING",
    "ECHO",
    "TIME",
    "LASTSAVE",
    "ROLE",
    "OBJECT",
    "MEMORY",
    "DUMP",
    "BITCOUNT",
    "BITPOS",
    "GETBIT",
    "PFCOUNT",
    "GEOPOS",
    "GEODIST",
    "GEOHASH",
    "GEOSEARCH",
    "GEORADIUS_RO",
    "GEORADIUSBYMEMBER_RO",
    "SORT_RO",
    "COMMAND",
    "SELECT",
    "JSON.GET",
    "JSON.MGET",
    "JSON.TYPE",
    "JSON.OBJKEYS",
    "JSON.OBJLEN",
    "JSON.STRLEN",
    "JSON.ARRLEN",
];

/// Commands that would hold the connection (subscriptions, replication streams).
const STREAMING_COMMANDS: &[&str] = &[
    "SUBSCRIBE",
    "PSUBSCRIBE",
    "SSUBSCRIBE",
    "MONITOR",
    "SYNC",
    "PSYNC",
    "QUIT",
    "RESET",
];

/// Turns a command's reply into grid rows, shaped by what the command returns.
fn reply_to_result(name: &str, args: &[Vec<u8>], reply: Reply) -> ResultSet {
    let has_arg = |a: &str| {
        args.iter()
            .skip(1)
            .any(|x| x.eq_ignore_ascii_case(a.as_bytes()))
    };
    let pairs = |items: Vec<Reply>, a: &str, b: &str| {
        let mut rows = Vec::new();
        let mut it = items.into_iter();
        while let (Some(x), Some(y)) = (it.next(), it.next()) {
            rows.push(vec![reply_val(x), reply_val(y)]);
        }
        ResultSet {
            columns: cols(&[a, b]),
            rows,
            ..Default::default()
        }
    };
    let mut rs = match (name, reply) {
        ("HGETALL", Reply::Array(a)) => pairs(a, "field", "value"),
        ("CONFIG", Reply::Array(a)) if has_arg("GET") => pairs(a, "parameter", "value"),
        (
            "ZRANGE" | "ZREVRANGE" | "ZRANGEBYSCORE" | "ZREVRANGEBYSCORE" | "ZRANDMEMBER",
            Reply::Array(a),
        ) if has_arg("WITHSCORES") => pairs(a, "member", "score"),
        ("XRANGE" | "XREVRANGE", Reply::Array(a)) => ResultSet {
            columns: cols(&["id", "fields"]),
            rows: stream_rows(a),
            ..Default::default()
        },
        ("SCAN" | "SSCAN" | "HSCAN" | "ZSCAN", Reply::Array(a)) => {
            let mut it = a.into_iter();
            let cursor = it.next().and_then(|c| c.text()).unwrap_or_default();
            let items = it.next().map(Reply::array).unwrap_or_default();
            let note = format!("next cursor {cursor}");
            match name {
                "HSCAN" => pairs(items, "field", &format!("value ({note})")),
                "ZSCAN" => pairs(items, "member", &format!("score ({note})")),
                _ => ResultSet {
                    columns: vec![Column::new(format!(
                        "{} ({note})",
                        if name == "SCAN" { "key" } else { "member" }
                    ))],
                    rows: items.into_iter().map(|r| vec![reply_val(r)]).collect(),
                    ..Default::default()
                },
            }
        }
        ("INFO", r @ (Reply::Bulk(_) | Reply::Status(_))) => {
            let text = r.text().unwrap_or_default();
            let mut section = String::new();
            let mut rows = Vec::new();
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if let Some(s) = line.strip_prefix('#') {
                    section = s.trim().to_string();
                } else if let Some((k, v)) = line.split_once(':') {
                    rows.push(vec![
                        Value::Text(section.clone()),
                        Value::Text(k.into()),
                        Value::Text(v.into()),
                    ]);
                }
            }
            ResultSet {
                columns: cols(&["section", "key", "value"]),
                rows,
                ..Default::default()
            }
        }
        (_, Reply::Array(a)) => ResultSet {
            columns: cols(&["#", "value"]),
            rows: a
                .into_iter()
                .enumerate()
                .map(|(i, r)| vec![Value::Int(i as i64 + 1), reply_val(r)])
                .collect(),
            ..Default::default()
        },
        (_, r) => ResultSet {
            columns: cols(&["reply"]),
            rows: vec![vec![reply_val(r)]],
            ..Default::default()
        },
    };
    rs.rows.truncate(ROW_CAP);
    rs
}

impl Session for RedisSession {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<ResultSet> {
        let args = split_args(sql).map_err(Error::InvalidParams)?;
        let Some(first) = args.first() else {
            return Err(Error::InvalidParams("empty command".into()));
        };
        let name = String::from_utf8_lossy(first).to_ascii_uppercase();
        if STREAMING_COMMANDS.contains(&name.as_str()) {
            return Err(Error::Unsupported(format!(
                "{name} keeps the connection busy; it cannot run from the query editor"
            )));
        }
        let sub = args
            .get(1)
            .map(|a| String::from_utf8_lossy(a).to_ascii_uppercase())
            .unwrap_or_default();
        let read = READ_COMMANDS.contains(&name.as_str())
            || (name == "CONFIG" && sub == "GET")
            || (name == "CLIENT" && matches!(sub.as_str(), "LIST" | "INFO" | "GETNAME" | "ID"));
        if self.read_only && !read {
            return Err(Error::ReadOnly(name));
        }
        let reply = self.cmd_owned(&args)?;
        if name == "SELECT" {
            if let Some(n) = args
                .get(1)
                .and_then(|a| std::str::from_utf8(a).ok()?.parse().ok())
            {
                self.db = n;
            }
        }
        Ok(reply_to_result(&name, &args, reply))
    }

    fn scan_keys(&mut self, scan: &KeyScan) -> Option<Result<KeyPage>> {
        Some(self.scan_step(scan))
    }

    fn ping(&mut self) -> Result<()> {
        self.cmd(&[b"PING"]).map(|_| ())
    }

    // Staged edits run in MULTI / EXEC (see `apply_changes`); SQL transactions do not apply.
    fn begin(&mut self) -> Result<()> {
        Ok(())
    }
    fn commit(&mut self) -> Result<()> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<()> {
        Ok(())
    }

    fn list_databases(&mut self) -> Result<Vec<String>> {
        // Managed servers often refuse CONFIG; 16 is the default number of databases.
        let n = self
            .cmd(&[b"CONFIG", b"GET", b"databases"])
            .ok()
            .and_then(|r| r.array().get(1).and_then(Reply::int))
            .unwrap_or(16)
            .clamp(1, 256);
        Ok((0..n).map(|i| format!("db{i}")).collect())
    }

    fn current_database(&mut self) -> Result<Option<String>> {
        Ok(Some(format!("db{}", self.db)))
    }

    fn list_objects(&mut self) -> Result<Vec<SchemaObject>> {
        let mut keys = Vec::new();
        let mut cursor = b"0".to_vec();
        loop {
            let mut parts = self
                .cmd(&[b"SCAN", &cursor, b"COUNT", b"1000"])?
                .array()
                .into_iter();
            cursor = parts
                .next()
                .and_then(|r| r.bytes())
                .unwrap_or(b"0".to_vec());
            for k in parts.next().map(Reply::array).unwrap_or_default() {
                if let Some(t) = k.text() {
                    keys.push(t);
                }
            }
            if cursor == b"0" || keys.len() >= KEY_LIMIT {
                break;
            }
        }
        keys.sort();
        keys.dedup();
        keys.truncate(KEY_LIMIT);
        Ok(keys
            .into_iter()
            .map(|k| SchemaObject {
                object: ObjectRef::new(k),
                kind: ObjectKind::Table,
                parent: None,
            })
            .collect())
    }

    fn describe_table(&mut self, t: &ObjectRef) -> Result<TableInfo> {
        let ty = self.key_type(&t.name)?;
        let spec: &[(&str, &str)] = match ty.as_str() {
            "string" => &[("value", "string")],
            "hash" => &[("field", "string"), ("value", "string")],
            "list" => &[("index", "integer"), ("value", "string")],
            "set" => &[("member", "string")],
            "zset" => &[("member", "string"), ("score", "double")],
            "stream" => &[("id", "stream id"), ("fields", "json")],
            "none" => return Err(Error::Database(format!("key `{}` does not exist", t.name))),
            other => &[(
                "value",
                if other.is_empty() {
                    "unknown"
                } else {
                    "module type"
                },
            )],
        };
        let keyed = ty != "string" && spec.len() > 1 || ty == "set";
        Ok(TableInfo {
            object: t.clone(),
            columns: spec
                .iter()
                .enumerate()
                .map(|(i, (n, dt))| ColumnInfo {
                    name: n.to_string(),
                    data_type: dt.to_string(),
                    nullable: false,
                    default: None,
                    primary_key: (keyed && i == 0).then_some(1),
                    comment: None,
                    auto_increment: false,
                })
                .collect(),
            foreign_keys: vec![],
            indexes: vec![],
            triggers: vec![],
        })
    }

    /// The key as commands that recreate it (first 100 items), with its type, size and
    /// TTL as comments.
    fn ddl(&mut self, o: &ObjectRef) -> Result<String> {
        let key = o.name.as_bytes();
        let q = quote_arg(&o.name);
        let ty = self.key_type(&o.name)?;
        let ttl = self.cmd(&[b"TTL", key])?.int().unwrap_or(-1);
        let (len_cmd, unit): (&[u8], &str) = match ty.as_str() {
            "string" => (b"STRLEN", "bytes"),
            "hash" => (b"HLEN", "fields"),
            "list" => (b"LLEN", "items"),
            "set" => (b"SCARD", "members"),
            "zset" => (b"ZCARD", "members"),
            "stream" => (b"XLEN", "entries"),
            _ => (b"EXISTS", "key"),
        };
        let len = self.cmd(&[len_cmd, key])?.int().unwrap_or(0);
        let enc = self
            .cmd(&[b"OBJECT", b"ENCODING", key])
            .ok()
            .and_then(|r| r.text())
            .unwrap_or_default();
        let mut out = format!(
            "# {ty} · {len} {unit} · TTL {} · encoding {enc}\n",
            if ttl < 0 {
                "none".into()
            } else {
                format!("{ttl} s")
            }
        );
        let a = |r: Reply| quote_arg(&r.text().unwrap_or_default());
        const N: &[u8] = b"99";
        match ty.as_str() {
            "string" => {
                let v = self.cmd(&[b"GET", key])?;
                out.push_str(&format!("SET {q} {}\n", a(v)));
            }
            "hash" => {
                let (items, _) = self.scan_page(b"HSCAN", &o.name, None, 2, (0, 100), false)?;
                for mut it in items {
                    let v = it.pop().map(a).unwrap_or_default();
                    let f = it.pop().map(a).unwrap_or_default();
                    out.push_str(&format!("HSET {q} {f} {v}\n"));
                }
            }
            "list" => {
                for v in self.cmd(&[b"LRANGE", key, b"0", N])?.array() {
                    out.push_str(&format!("RPUSH {q} {}\n", a(v)));
                }
            }
            "set" => {
                let (items, _) = self.scan_page(b"SSCAN", &o.name, None, 1, (0, 100), false)?;
                for mut it in items {
                    out.push_str(&format!(
                        "SADD {q} {}\n",
                        it.pop().map(a).unwrap_or_default()
                    ));
                }
            }
            "zset" => {
                let items = self.cmd(&[b"ZRANGE", key, b"0", N, b"WITHSCORES"])?.array();
                let mut it = items.into_iter();
                while let (Some(m), Some(s)) = (it.next(), it.next()) {
                    out.push_str(&format!("ZADD {q} {} {}\n", a(s), a(m)));
                }
            }
            "stream" => {
                for row in stream_rows(
                    self.cmd(&[b"XRANGE", key, b"-", b"+", b"COUNT", b"100"])?
                        .array(),
                ) {
                    out.push_str(&format!("# {} {}\n", row[0], row[1]));
                }
            }
            _ => {}
        }
        if len > 100 && ty != "string" {
            out.push_str(&format!("# … first 100 of {len} shown\n"));
        }
        if ttl > 0 {
            out.push_str(&format!("EXPIRE {q} {ttl}\n"));
        }
        Ok(out)
    }

    fn fetch_page(&mut self, req: &PageRequest, paged: bool) -> Option<Result<(ResultSet, u64)>> {
        Some((|| {
            let key = req.table.name.clone();
            let kb = key.as_bytes();
            let ty = self.key_type(&key)?;
            let size = if paged {
                req.page_size.max(1)
            } else {
                u64::MAX
            };
            let offset = if paged { req.page * size } else { 0 };
            let first_col = match ty.as_str() {
                "hash" => "field",
                "set" | "zset" => "member",
                _ => "",
            };
            let pattern = if first_col.is_empty() {
                if !req.filters.is_empty()
                    || req
                        .raw_where
                        .as_deref()
                        .is_some_and(|r| !r.trim().is_empty())
                {
                    return Err(Error::Unsupported(format!("filters on a {ty} key")));
                }
                None
            } else {
                match_pattern(&req.filters, req.raw_where.as_deref(), first_col)?
            };
            let stop = offset
                .saturating_add(size)
                .saturating_sub(1)
                .min(i64::MAX as u64);
            let (start_s, stop_s) = (offset.to_string(), stop.to_string());
            let (columns, rows, total, shown): (Vec<Column>, Vec<Vec<Value>>, u64, String) =
                match ty.as_str() {
                    "string" => {
                        let v = self.cmd(&[b"GET", kb])?;
                        (
                            cols(&["value"]),
                            vec![vec![reply_val(v)]],
                            1,
                            format!("GET {}", quote_arg(&key)),
                        )
                    }
                    "list" => {
                        let total = self.cmd(&[b"LLEN", kb])?.int().unwrap_or(0) as u64;
                        let items = self
                            .cmd(&[b"LRANGE", kb, start_s.as_bytes(), stop_s.as_bytes()])?
                            .array();
                        let rows = items
                            .into_iter()
                            .enumerate()
                            .map(|(i, r)| {
                                vec![Value::Int((offset + i as u64) as i64), reply_val(r)]
                            })
                            .collect();
                        (
                            cols(&["index", "value"]),
                            rows,
                            total,
                            format!("LRANGE {} {start_s} {stop_s}", quote_arg(&key)),
                        )
                    }
                    "zset" if pattern.is_none() => {
                        let total = self.cmd(&[b"ZCARD", kb])?.int().unwrap_or(0) as u64;
                        let desc = req
                            .sort
                            .first()
                            .is_some_and(|s| s.column == "score" && s.descending);
                        let c: &[u8] = if desc { b"ZREVRANGE" } else { b"ZRANGE" };
                        let items = self
                            .cmd(&[c, kb, start_s.as_bytes(), stop_s.as_bytes(), b"WITHSCORES"])?
                            .array();
                        let mut rows = Vec::new();
                        let mut it = items.into_iter();
                        while let (Some(m), Some(s)) = (it.next(), it.next()) {
                            let score = s.text().and_then(|t| t.parse::<f64>().ok());
                            rows.push(vec![reply_val(m), score.map_or(Value::Null, Value::Float)]);
                        }
                        let c = String::from_utf8_lossy(c).into_owned();
                        (
                            cols(&["member", "score"]),
                            rows,
                            total,
                            format!("{c} {} {start_s} {stop_s} WITHSCORES", quote_arg(&key)),
                        )
                    }
                    "hash" | "set" | "zset" => {
                        let (scan, per, names): (&[u8], usize, &[&str]) = match ty.as_str() {
                            "hash" => (b"HSCAN", 2, &["field", "value"]),
                            "set" => (b"SSCAN", 1, &["member"]),
                            _ => (b"ZSCAN", 2, &["member", "score"]),
                        };
                        let (items, seen) = self.scan_page(
                            scan,
                            &key,
                            pattern.as_deref(),
                            per,
                            (offset, size),
                            pattern.is_some(),
                        )?;
                        let total = match (pattern.is_some(), ty.as_str()) {
                            (true, _) => seen,
                            (_, "hash") => self.cmd(&[b"HLEN", kb])?.int().unwrap_or(0) as u64,
                            _ => self.cmd(&[b"SCARD", kb])?.int().unwrap_or(0) as u64,
                        };
                        let rows = items
                            .into_iter()
                            .map(|it| {
                                it.into_iter()
                                    .enumerate()
                                    .map(|(i, r)| match (ty.as_str(), i) {
                                        ("zset", 1) => r
                                            .text()
                                            .and_then(|t| t.parse::<f64>().ok())
                                            .map_or(Value::Null, Value::Float),
                                        _ => reply_val(r),
                                    })
                                    .collect()
                            })
                            .collect();
                        let scan = String::from_utf8_lossy(scan).into_owned();
                        let m = pattern
                            .as_deref()
                            .map(|p| format!(" MATCH {}", quote_arg(p)))
                            .unwrap_or_default();
                        (
                            cols(names),
                            rows,
                            total,
                            format!("{scan} {} 0{m}", quote_arg(&key)),
                        )
                    }
                    "stream" => {
                        let total = self.cmd(&[b"XLEN", kb])?.int().unwrap_or(0) as u64;
                        let count = offset.saturating_add(size).min(i64::MAX as u64).to_string();
                        let desc = req
                            .sort
                            .first()
                            .is_some_and(|s| s.column == "id" && s.descending);
                        let entries = if desc {
                            self.cmd(&[b"XREVRANGE", kb, b"+", b"-", b"COUNT", count.as_bytes()])?
                        } else {
                            self.cmd(&[b"XRANGE", kb, b"-", b"+", b"COUNT", count.as_bytes()])?
                        }
                        .array();
                        let rows = stream_rows(entries.into_iter().skip(offset as usize).collect());
                        (
                            cols(&["id", "fields"]),
                            rows,
                            total,
                            format!("XRANGE {} - + COUNT {count}", quote_arg(&key)),
                        )
                    }
                    "none" => return Err(Error::Database(format!("key `{key}` does not exist"))),
                    other => (
                        cols(&["value"]),
                        vec![vec![Value::Text(format!(
                            "({other} key: use the query editor)"
                        ))]],
                        1,
                        format!("TYPE {}", quote_arg(&key)),
                    ),
                };
            Ok((
                ResultSet {
                    columns,
                    rows,
                    sql: shown,
                    ..Default::default()
                },
                total,
            ))
        })())
    }

    fn preview_changes(&mut self, t: &ObjectRef, changes: &[StagedChange]) -> Option<Vec<String>> {
        Some(match self.plan(&t.name, changes) {
            Ok(cmds) => cmds
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|a| quote_arg(&String::from_utf8_lossy(a)))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect(),
            Err(e) => vec![format!("# {e}")],
        })
    }

    fn apply_changes(&mut self, t: &ObjectRef, changes: &[StagedChange]) -> Option<Result<u64>> {
        Some((|| {
            if self.read_only {
                return Err(Error::ReadOnly("staged changes".into()));
            }
            let cmds = self.plan(&t.name, changes)?;
            if cmds.is_empty() {
                return Ok(0);
            }
            self.cmd(&[b"MULTI"])?;
            for c in &cmds {
                let refs: Vec<&[u8]> = c.iter().map(Vec::as_slice).collect();
                if let Reply::Error(e) = self.raw(&refs)? {
                    self.raw(&[b"DISCARD"]).ok();
                    return Err(Error::Database(e));
                }
            }
            let results = self.cmd(&[b"EXEC"])?.array();
            if let Some(Reply::Error(e)) = results.iter().find(|r| matches!(r, Reply::Error(_))) {
                return Err(Error::Database(format!(
                    "{e} (commands before it were applied; Redis does not roll back)"
                )));
            }
            Ok(changes.len() as u64)
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names() {
        assert_eq!(parse_db("db3").unwrap(), 3);
        assert_eq!(parse_db("7").unwrap(), 7);
        assert_eq!(parse_db("").unwrap(), 0);
        assert!(parse_db("x").is_err());
    }

    #[test]
    fn filters_become_match_globs() {
        let like = Filter {
            column: "field".into(),
            op: FilterOp::Like,
            values: vec![Value::from("user_%*")],
        };
        assert_eq!(
            match_pattern(std::slice::from_ref(&like), None, "field")
                .unwrap()
                .as_deref(),
            Some("user?*\\*")
        );
        let eq = Filter {
            op: FilterOp::Eq,
            values: vec![Value::from("a[1]")],
            ..like.clone()
        };
        assert_eq!(
            match_pattern(&[eq], None, "field").unwrap().as_deref(),
            Some("a\\[1\\]")
        );
        assert_eq!(
            match_pattern(&[], Some(" u:* "), "field")
                .unwrap()
                .as_deref(),
            Some("u:*")
        );
        let gt = Filter {
            op: FilterOp::Gt,
            ..like
        };
        assert!(match_pattern(&[gt], None, "field").is_err());
    }

    #[test]
    fn replies_become_tables() {
        let args = |s: &str| split_args(s).unwrap();
        let hash = Reply::Array(vec![
            Reply::Bulk(b"name".to_vec()),
            Reply::Bulk(b"Ann".to_vec()),
            Reply::Bulk(b"age".to_vec()),
            Reply::Bulk(b"3".to_vec()),
        ]);
        let rs = reply_to_result("HGETALL", &args("HGETALL u"), hash);
        assert_eq!(rs.columns, cols(&["field", "value"]));
        assert_eq!(rs.rows[1], vec![Value::from("age"), Value::from("3")]);

        let scan = Reply::Array(vec![
            Reply::Bulk(b"17".to_vec()),
            Reply::Array(vec![Reply::Bulk(b"k1".to_vec())]),
        ]);
        let rs = reply_to_result("SCAN", &args("SCAN 0"), scan);
        assert_eq!(rs.columns[0].name, "key (next cursor 17)");

        let info = Reply::Bulk(
            b"# Server\r\nredis_version:7.4.0\r\n\r\n# Clients\r\nconnected_clients:1\r\n".to_vec(),
        );
        let rs = reply_to_result("INFO", &args("INFO"), info);
        assert_eq!(
            rs.rows[1],
            vec![
                Value::from("Clients"),
                Value::from("connected_clients"),
                Value::from("1")
            ]
        );

        let rs = reply_to_result("GET", &args("GET k"), Reply::Nil);
        assert_eq!(rs.rows, vec![vec![Value::Null]]);

        let stream = Reply::Array(vec![Reply::Array(vec![
            Reply::Bulk(b"1-0".to_vec()),
            Reply::Array(vec![
                Reply::Bulk(b"t".to_vec()),
                Reply::Bulk(b"21".to_vec()),
            ]),
        ])]);
        let rs = reply_to_result("XRANGE", &args("XRANGE s - +"), stream);
        assert_eq!(
            rs.rows[0],
            vec![Value::from("1-0"), Value::from("{\"t\":\"21\"}")]
        );
    }

    fn live() -> Option<ConnectionConfig> {
        let url = std::env::var("THAAN_TEST_REDIS_URL").ok()?;
        ConnectionConfig::from_url(&url).ok()
    }

    /// Needs a server: THAAN_TEST_REDIS_URL=redis://127.0.0.1:56379/15
    #[test]
    #[ignore]
    fn live_key_views_and_edits() {
        let cfg = live().expect("THAAN_TEST_REDIS_URL");
        let mut s = RedisDriver.connect(&cfg).unwrap();
        s.execute("FLUSHDB", &[]).unwrap();
        s.execute("HSET user:1 name Ann age 3", &[]).unwrap();
        s.execute("RPUSH jobs a b c d", &[]).unwrap();
        s.execute("ZADD board 10 x 20 y", &[]).unwrap();
        s.execute("SET greeting \"hello world\"", &[]).unwrap();
        s.execute("XADD events * kind click", &[]).unwrap();
        let names: Vec<_> = s
            .list_objects()
            .unwrap()
            .into_iter()
            .map(|o| o.object.name)
            .collect();
        assert_eq!(names, ["board", "events", "greeting", "jobs", "user:1"]);

        let page = |s: &mut Box<dyn Session>, key: &str, page: u64, size: u64| {
            let req = PageRequest {
                table: ObjectRef::new(key),
                filters: vec![],
                raw_where: None,
                projection: None,
                raw_sort: None,
                sort: vec![],
                page,
                page_size: size,
            };
            s.fetch_page(&req, true).unwrap().unwrap()
        };
        let (rs, total) = page(&mut s, "jobs", 1, 3);
        assert_eq!(total, 4);
        assert_eq!(rs.rows, vec![vec![Value::Int(3), Value::from("d")]]);
        let (rs, total) = page(&mut s, "user:1", 0, 50);
        assert_eq!((total, rs.rows.len()), (2, 2));
        let (rs, _) = page(&mut s, "board", 0, 50);
        assert_eq!(rs.rows[1], vec![Value::from("y"), Value::Float(20.0)]);

        // Delete two list items by index and edit a hash field, in one MULTI / EXEC.
        let n = s
            .apply_changes(
                &ObjectRef::new("jobs"),
                &[
                    StagedChange::Delete {
                        key: vec![("index".into(), Value::Int(0))],
                    },
                    StagedChange::Delete {
                        key: vec![("index".into(), Value::Int(2))],
                    },
                ],
            )
            .unwrap()
            .unwrap();
        assert_eq!(n, 2);
        let (rs, _) = page(&mut s, "jobs", 0, 50);
        assert_eq!(
            rs.rows.iter().map(|r| r[1].clone()).collect::<Vec<_>>(),
            [Value::from("b"), Value::from("d")]
        );
        s.apply_changes(
            &ObjectRef::new("user:1"),
            &[StagedChange::Update {
                key: vec![("field".into(), Value::from("age"))],
                set: vec![("field".into(), Value::from("years"))],
            }],
        )
        .unwrap()
        .unwrap();
        let rs = s.execute("HGET user:1 years", &[]).unwrap();
        assert_eq!(rs.rows[0][0], Value::from("3"));
        assert!(s
            .ddl(&ObjectRef::new("greeting"))
            .unwrap()
            .contains("SET greeting \"hello world\""));

        let page = s
            .scan_keys(&KeyScan {
                pattern: Some("user:*".into()),
                cursor: "0".into(),
                count: 100,
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(page.keys.len(), 1);
        assert_eq!(
            (
                page.keys[0].name.as_str(),
                page.keys[0].key_type.as_str(),
                page.keys[0].ttl
            ),
            ("user:1", "hash", None)
        );
        s.execute("EXPIRE jobs 100", &[]).unwrap();
        let page = s
            .scan_keys(&KeyScan {
                key_type: Some("list".into()),
                count: 100,
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(page.keys[0].name, "jobs");
        assert!(page.keys[0].ttl.is_some_and(|t| t > 0 && t <= 100));
        assert_eq!(page.total, Some(5));

        let mut ro = cfg.clone();
        ro.read_only = true;
        let mut r = RedisDriver.connect(&ro).unwrap();
        assert!(r.execute("GET greeting", &[]).is_ok());
        assert!(matches!(
            r.execute("DEL greeting", &[]),
            Err(Error::ReadOnly(_))
        ));
        s.execute("FLUSHDB", &[]).unwrap();
    }
}
