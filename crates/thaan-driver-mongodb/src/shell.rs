//! A small parser for the MongoDB shell syntax people type into a query editor:
//!
//! ```text
//! db.users.find({ age: { $gt: 30 } }, { name: 1 }).sort({ name: 1 }).skip(10).limit(5)
//! db.getCollection("my users").aggregate([{ $group: { _id: "$country", n: { $sum: 1 } } }])
//! db.users.updateMany({ active: false }, { $set: { archived: true } })
//! show collections
//! ```
//!
//! Values are relaxed JSON / JavaScript literals: unquoted keys, single or double quotes,
//! trailing commas, comments, `/regex/flags`, and the shell helpers `ObjectId()`,
//! `ISODate()` / `new Date()`, `NumberInt()`, `NumberLong()`, `NumberDecimal()`, `UUID()`.
//! Written from the MongoDB shell documentation.

use mongodb::bson::{self, oid::ObjectId, spec::BinarySubtype, Binary, Bson, Document, Regex};

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Find {
        coll: String,
        filter: Document,
        projection: Option<Document>,
        sort: Option<Document>,
        skip: Option<u64>,
        limit: Option<i64>,
        one: bool,
    },
    Count {
        coll: String,
        filter: Document,
    },
    Distinct {
        coll: String,
        field: String,
        filter: Document,
    },
    Aggregate {
        coll: String,
        pipeline: Vec<Document>,
    },
    InsertOne {
        coll: String,
        doc: Document,
    },
    InsertMany {
        coll: String,
        docs: Vec<Document>,
    },
    Update {
        coll: String,
        filter: Document,
        update: UpdateSpec,
        many: bool,
        upsert: bool,
    },
    Replace {
        coll: String,
        filter: Document,
        doc: Document,
        upsert: bool,
    },
    Delete {
        coll: String,
        filter: Document,
        many: bool,
    },
    CreateIndex {
        coll: String,
        keys: Document,
        options: Document,
    },
    GetIndexes {
        coll: String,
    },
    DropCollection {
        coll: String,
    },
    CreateCollection {
        name: String,
        options: Document,
    },
    ListCollections,
    ListDatabases,
    RunCommand {
        command: Document,
    },
}

/// An update is a document of operators or an aggregation pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateSpec {
    Doc(Document),
    Pipeline(Vec<Document>),
}

/// Commands that only read, for `runCommand` on read-only connections.
const READ_COMMANDS: &[&str] = &[
    "ping",
    "buildinfo",
    "buildInfo",
    "serverStatus",
    "dbStats",
    "dbstats",
    "collStats",
    "collstats",
    "listCollections",
    "listIndexes",
    "listDatabases",
    "count",
    "distinct",
    "find",
    "hello",
    "isMaster",
    "ismaster",
    "hostInfo",
    "connectionStatus",
    "explain",
    "getParameter",
];

impl Op {
    pub fn is_write(&self) -> bool {
        match self {
            Op::Find { .. }
            | Op::Count { .. }
            | Op::Distinct { .. }
            | Op::GetIndexes { .. }
            | Op::ListCollections
            | Op::ListDatabases => false,
            Op::Aggregate { pipeline, .. } => pipeline
                .iter()
                .any(|stage| stage.contains_key("$out") || stage.contains_key("$merge")),
            Op::RunCommand { command } => command
                .keys()
                .next()
                .is_none_or(|k| !READ_COMMANDS.contains(&k.as_str())),
            _ => true,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Op::Find { one: true, .. } => "findOne",
            Op::Find { .. } => "find",
            Op::Count { .. } => "countDocuments",
            Op::Distinct { .. } => "distinct",
            Op::Aggregate { .. } => "aggregate",
            Op::InsertOne { .. } => "insertOne",
            Op::InsertMany { .. } => "insertMany",
            Op::Update { many: true, .. } => "updateMany",
            Op::Update { .. } => "updateOne",
            Op::Replace { .. } => "replaceOne",
            Op::Delete { many: true, .. } => "deleteMany",
            Op::Delete { .. } => "deleteOne",
            Op::CreateIndex { .. } => "createIndex",
            Op::GetIndexes { .. } => "getIndexes",
            Op::DropCollection { .. } => "drop",
            Op::CreateCollection { .. } => "createCollection",
            Op::ListCollections => "getCollectionNames",
            Op::ListDatabases => "show dbs",
            Op::RunCommand { .. } => "runCommand",
        }
    }
}

pub type ParseResult<T> = std::result::Result<T, String>;

struct P<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> P<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, i: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn ws(&mut self) {
        loop {
            let r = self.rest();
            if let Some(c) = r.chars().next().filter(|c| c.is_whitespace()) {
                self.i += c.len_utf8();
            } else if r.starts_with("//") {
                self.i += r.find('\n').unwrap_or(r.len());
            } else if r.starts_with("/*") {
                self.i += r.find("*/").map(|p| p + 2).unwrap_or(r.len());
            } else {
                break;
            }
        }
    }

    fn err<T>(&self, what: &str) -> ParseResult<T> {
        let near: String = self.rest().chars().take(20).collect();
        if near.is_empty() {
            Err(format!("{what} at end of input"))
        } else {
            Err(format!("{what} near `{near}`"))
        }
    }

    fn eat(&mut self, c: char) -> bool {
        self.ws();
        if self.peek() == Some(c) {
            self.i += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> ParseResult<()> {
        if self.eat(c) {
            Ok(())
        } else {
            self.err(&format!("expected `{c}`"))
        }
    }

    fn ident(&mut self) -> Option<&'a str> {
        self.ws();
        let r = self.rest();
        let len = r
            .char_indices()
            .take_while(|(n, c)| {
                c.is_alphanumeric() || *c == '_' || *c == '$' || (*n > 0 && c.is_ascii_digit())
            })
            .map(|(n, c)| n + c.len_utf8())
            .last()?;
        if r.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        self.i += len;
        Some(&r[..len])
    }

    fn string(&mut self) -> ParseResult<String> {
        self.ws();
        let q = match self.peek() {
            Some(c @ ('"' | '\'')) => c,
            _ => return self.err("expected a string"),
        };
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return self.err("unterminated string"),
                Some(c) if c == q => return Ok(out),
                Some('\\') => match self.bump() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some('0') => out.push('\0'),
                    Some('u') => {
                        let hex: String = (0..4).filter_map(|_| self.bump()).collect();
                        let c = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
                        match c {
                            Some(c) => out.push(c),
                            None => return self.err("bad \\u escape"),
                        }
                    }
                    Some(c) => out.push(c),
                    None => return self.err("unterminated string"),
                },
                Some(c) => out.push(c),
            }
        }
    }

    fn number(&mut self) -> ParseResult<Bson> {
        self.ws();
        let r = self.rest();
        let len = r
            .char_indices()
            .take_while(|(n, c)| {
                c.is_ascii_digit()
                    || matches!(c, '.' | 'e' | 'E' | '_')
                    || (matches!(c, '-' | '+') && (*n == 0 || r[..*n].ends_with(['e', 'E'])))
            })
            .map(|(n, c)| n + c.len_utf8())
            .last()
            .unwrap_or(0);
        let text = r[..len].replace('_', "");
        if len == 0 {
            return self.err("expected a number");
        }
        self.i += len;
        if !text.contains(['.', 'e', 'E']) {
            if let Ok(n) = text.parse::<i64>() {
                return Ok(i32::try_from(n).map(Bson::Int32).unwrap_or(Bson::Int64(n)));
            }
        }
        match text.parse::<f64>() {
            Ok(f) => Ok(Bson::Double(f)),
            Err(_) => self.err("bad number"),
        }
    }

    fn args(&mut self) -> ParseResult<Vec<Bson>> {
        self.expect('(')?;
        let mut out = Vec::new();
        loop {
            if self.eat(')') {
                return Ok(out);
            }
            out.push(self.value()?);
            if !self.eat(',') {
                self.expect(')')?;
                return Ok(out);
            }
        }
    }

    fn value(&mut self) -> ParseResult<Bson> {
        self.ws();
        match self.peek() {
            Some('{') => self.document().map(Bson::Document),
            Some('[') => {
                self.bump();
                let mut arr = Vec::new();
                loop {
                    if self.eat(']') {
                        return Ok(Bson::Array(arr));
                    }
                    arr.push(self.value()?);
                    if !self.eat(',') {
                        self.expect(']')?;
                        return Ok(Bson::Array(arr));
                    }
                }
            }
            Some('"' | '\'') => self.string().map(Bson::String),
            Some('/') => {
                self.bump();
                let mut pattern = String::new();
                loop {
                    match self.bump() {
                        None => return self.err("unterminated regex"),
                        Some('\\') => {
                            pattern.push('\\');
                            if let Some(c) = self.bump() {
                                pattern.push(c);
                            }
                        }
                        Some('/') => break,
                        Some(c) => pattern.push(c),
                    }
                }
                let flags: String = std::iter::from_fn(|| {
                    self.peek()
                        .filter(|c| c.is_ascii_alphabetic())
                        .and_then(|_| self.bump())
                })
                .collect();
                let mut opts: Vec<char> = flags.chars().collect();
                opts.sort_unstable();
                Ok(Bson::RegularExpression(Regex {
                    pattern,
                    options: opts.into_iter().collect(),
                }))
            }
            Some(c) if c.is_ascii_digit() || c == '-' || c == '.' => self.number(),
            Some(_) => {
                let Some(word) = self.ident() else {
                    return self.err("expected a value");
                };
                match word {
                    "true" => Ok(Bson::Boolean(true)),
                    "false" => Ok(Bson::Boolean(false)),
                    "null" | "undefined" => Ok(Bson::Null),
                    "Infinity" => Ok(Bson::Double(f64::INFINITY)),
                    "NaN" => Ok(Bson::Double(f64::NAN)),
                    "new" => self.value(),
                    helper => self.helper(helper),
                }
            }
            None => self.err("expected a value"),
        }
    }

    fn helper(&mut self, name: &str) -> ParseResult<Bson> {
        let args = self.args()?;
        let first_str = || match args.first() {
            Some(Bson::String(s)) => Some(s.clone()),
            _ => None,
        };
        match name {
            "ObjectId" => match first_str() {
                Some(hex) => ObjectId::parse_str(&hex)
                    .map(Bson::ObjectId)
                    .map_err(|e| format!("ObjectId: {e}")),
                None => Ok(Bson::ObjectId(ObjectId::new())),
            },
            "ISODate" | "Date" => match first_str() {
                Some(s) => parse_date(&s).map(Bson::DateTime),
                None => match args.first() {
                    Some(Bson::Int32(n)) => {
                        Ok(Bson::DateTime(bson::DateTime::from_millis(*n as i64)))
                    }
                    Some(Bson::Int64(n)) => Ok(Bson::DateTime(bson::DateTime::from_millis(*n))),
                    _ => Ok(Bson::DateTime(bson::DateTime::now())),
                },
            },
            "NumberInt" => int_arg(&args).and_then(|n| {
                i32::try_from(n)
                    .map(Bson::Int32)
                    .map_err(|_| "NumberInt out of range".into())
            }),
            "NumberLong" => int_arg(&args).map(Bson::Int64),
            "NumberDecimal" => {
                let text = first_str()
                    .or_else(|| args.first().map(|b| b.to_string()))
                    .unwrap_or_default();
                text.parse::<bson::Decimal128>()
                    .map(Bson::Decimal128)
                    .map_err(|e| format!("NumberDecimal: {e}"))
            }
            "UUID" => {
                let s = first_str().ok_or("UUID expects a string")?;
                let hex: String = s.chars().filter(|c| *c != '-').collect();
                if hex.len() != 32 {
                    return Err("UUID expects 32 hex digits".into());
                }
                let bytes = (0..16)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|_| "UUID expects hex digits".to_string())?;
                Ok(Bson::Binary(Binary {
                    subtype: BinarySubtype::Uuid,
                    bytes,
                }))
            }
            other => Err(format!("unknown helper `{other}()`")),
        }
    }

    fn document(&mut self) -> ParseResult<Document> {
        self.expect('{')?;
        let mut doc = Document::new();
        loop {
            if self.eat('}') {
                return Ok(doc);
            }
            self.ws();
            let key = match self.peek() {
                Some('"' | '\'') => self.string()?,
                Some(c) if c.is_ascii_digit() => match self.number()? {
                    Bson::Int32(n) => n.to_string(),
                    other => other.to_string(),
                },
                _ => match self.ident() {
                    Some(k) => k.to_string(),
                    None => return self.err("expected a key"),
                },
            };
            self.expect(':')?;
            let v = self.value()?;
            doc.insert(key, v);
            if !self.eat(',') {
                self.expect('}')?;
                return Ok(doc);
            }
        }
    }
}

fn int_arg(args: &[Bson]) -> ParseResult<i64> {
    match args.first() {
        Some(Bson::Int32(n)) => Ok(*n as i64),
        Some(Bson::Int64(n)) => Ok(*n),
        Some(Bson::Double(f)) if f.fract() == 0.0 => Ok(*f as i64),
        Some(Bson::String(s)) => s.trim().parse().map_err(|_| format!("not an integer: {s}")),
        _ => Err("expected an integer".into()),
    }
}

fn parse_date(s: &str) -> ParseResult<bson::DateTime> {
    let s = s.trim();
    // Accept date-only and space-separated forms as UTC.
    let full = if s.len() == 10 {
        format!("{s}T00:00:00Z")
    } else {
        let t = s.replacen(' ', "T", 1);
        if t.ends_with('Z') || t[10..].contains(['+', '-']) {
            t
        } else {
            format!("{t}Z")
        }
    };
    bson::DateTime::parse_rfc3339_str(&full).map_err(|e| format!("ISODate(\"{s}\"): {e}"))
}

/// Parses one relaxed-JSON value (used for filters typed in the table view).
pub fn parse_value(text: &str) -> ParseResult<Bson> {
    let mut p = P::new(text);
    let v = p.value()?;
    p.ws();
    if !p.rest().is_empty() {
        return p.err("unexpected text after value");
    }
    Ok(v)
}

/// Parses a relaxed-JSON document; empty text is `{}`.
pub fn parse_document(text: &str) -> ParseResult<Document> {
    if text.trim().is_empty() {
        return Ok(Document::new());
    }
    match parse_value(text)? {
        Bson::Document(d) => Ok(d),
        _ => Err("expected a document like { field: value }".into()),
    }
}

fn as_doc(v: Option<&Bson>, what: &str) -> ParseResult<Document> {
    match v {
        None => Ok(Document::new()),
        Some(Bson::Document(d)) => Ok(d.clone()),
        Some(_) => Err(format!("{what} must be a document")),
    }
}

fn as_docs(v: Option<&Bson>, what: &str) -> ParseResult<Vec<Document>> {
    match v {
        Some(Bson::Array(a)) => a
            .iter()
            .map(|b| match b {
                Bson::Document(d) => Ok(d.clone()),
                _ => Err(format!("{what} must be an array of documents")),
            })
            .collect(),
        _ => Err(format!("{what} must be an array of documents")),
    }
}

fn upsert(opts: Option<&Bson>) -> bool {
    matches!(opts, Some(Bson::Document(d)) if d.get_bool("upsert").unwrap_or(false))
}

/// Parses one shell command.
pub fn parse(input: &str) -> ParseResult<Op> {
    let mut p = P::new(input.trim().trim_end_matches(';'));
    p.ws();
    let first = p.ident().ok_or_else(|| {
        "expected `db.<collection>.<method>(…)` or `show collections`".to_string()
    })?;
    match first {
        "show" => {
            let what = p.ident().unwrap_or_default();
            return match what {
                "collections" | "tables" => Ok(Op::ListCollections),
                "dbs" | "databases" => Ok(Op::ListDatabases),
                _ => Err(format!("unknown `show {what}`")),
            };
        }
        "use" => {
            return Err(
                "`use` is not supported here; open a connection to that database instead".into(),
            )
        }
        "db" => {}
        other => return Err(format!("expected `db`, found `{other}`")),
    }

    // Collection (or a db-level method).
    let coll: String = if p.eat('[') {
        let name = p.string()?;
        p.expect(']')?;
        name
    } else {
        p.expect('.')?;
        let name = p
            .ident()
            .ok_or("expected a collection name after `db.`")?
            .to_string();
        p.ws();
        if p.peek() == Some('(') {
            let args = p.args()?;
            let s0 = match args.first() {
                Some(Bson::String(s)) => Some(s.clone()),
                _ => None,
            };
            match name.as_str() {
                "getCollection" => s0.ok_or("getCollection expects a name")?,
                "getCollectionNames" | "getCollectionInfos" => {
                    return finish(&mut p, Op::ListCollections)
                }
                "createCollection" => {
                    let name = s0.ok_or("createCollection expects a name")?;
                    return finish(
                        &mut p,
                        Op::CreateCollection {
                            name,
                            options: as_doc(args.get(1), "options")?,
                        },
                    );
                }
                "runCommand" | "adminCommand" => {
                    return finish(
                        &mut p,
                        Op::RunCommand {
                            command: as_doc(args.first(), "command")?,
                        },
                    )
                }
                "stats" => {
                    return finish(
                        &mut p,
                        Op::RunCommand {
                            command: bson::doc! { "dbStats": 1 },
                        },
                    )
                }
                other => return Err(format!("unsupported database method `db.{other}()`")),
            }
        } else {
            name
        }
    };

    p.expect('.')?;
    let method = p.ident().ok_or("expected a method name")?;
    let args = p.args()?;
    let a = |i: usize| args.get(i);
    let mut op = match method {
        "find" | "findOne" => Op::Find {
            coll,
            filter: as_doc(a(0), "filter")?,
            projection: a(1).map(|b| as_doc(Some(b), "projection")).transpose()?,
            sort: None,
            skip: None,
            limit: if method == "findOne" { Some(1) } else { None },
            one: method == "findOne",
        },
        "countDocuments" | "count" => Op::Count {
            coll,
            filter: as_doc(a(0), "filter")?,
        },
        "estimatedDocumentCount" => Op::Count {
            coll,
            filter: Document::new(),
        },
        "distinct" => Op::Distinct {
            coll,
            field: match a(0) {
                Some(Bson::String(s)) => s.clone(),
                _ => return Err("distinct expects a field name".into()),
            },
            filter: as_doc(a(1), "filter")?,
        },
        "aggregate" => Op::Aggregate {
            coll,
            pipeline: as_docs(a(0), "pipeline")?,
        },
        "insertOne" | "insert" if matches!(a(0), Some(Bson::Document(_))) => Op::InsertOne {
            coll,
            doc: as_doc(a(0), "document")?,
        },
        "insertMany" | "insert" => Op::InsertMany {
            coll,
            docs: as_docs(a(0), "documents")?,
        },
        "updateOne" | "updateMany" => Op::Update {
            coll,
            filter: as_doc(a(0), "filter")?,
            update: match a(1) {
                Some(Bson::Document(d)) => UpdateSpec::Doc(d.clone()),
                Some(b @ Bson::Array(_)) => {
                    UpdateSpec::Pipeline(as_docs(Some(b), "update pipeline")?)
                }
                _ => return Err("update expects an update document or pipeline".into()),
            },
            many: method == "updateMany",
            upsert: upsert(a(2)),
        },
        "replaceOne" => Op::Replace {
            coll,
            filter: as_doc(a(0), "filter")?,
            doc: as_doc(a(1), "replacement")?,
            upsert: upsert(a(2)),
        },
        "deleteOne" | "deleteMany" | "remove" => Op::Delete {
            coll,
            filter: as_doc(a(0), "filter")?,
            many: method != "deleteOne",
        },
        "createIndex" => Op::CreateIndex {
            coll,
            keys: as_doc(a(0), "keys")?,
            options: as_doc(a(1), "options")?,
        },
        "getIndexes" => Op::GetIndexes { coll },
        "drop" => Op::DropCollection { coll },
        other => return Err(format!("unsupported method `.{other}()`")),
    };

    // Cursor modifiers.
    while p.eat('.') {
        let m = p.ident().ok_or("expected a cursor method")?;
        let args = p.args()?;
        match (&mut op, m) {
            (Op::Find { sort, .. }, "sort") => *sort = Some(as_doc(args.first(), "sort")?),
            (Op::Find { projection, .. }, "projection") => {
                *projection = Some(as_doc(args.first(), "projection")?)
            }
            (Op::Find { limit, .. }, "limit") => *limit = Some(int_arg(&args)?),
            (Op::Find { skip, .. }, "skip") => *skip = Some(int_arg(&args)?.max(0) as u64),
            (Op::Find { coll, filter, .. }, "count" | "itcount") => {
                op = Op::Count {
                    coll: coll.clone(),
                    filter: filter.clone(),
                };
            }
            (_, "pretty" | "toArray") => {}
            (_, other) => return Err(format!("unsupported cursor method `.{other}()`")),
        }
    }
    finish(&mut p, op)
}

fn finish(p: &mut P<'_>, op: Op) -> ParseResult<Op> {
    p.ws();
    if p.rest().is_empty() {
        Ok(op)
    } else {
        p.err("unexpected text after the command")
    }
}

/// Shell-style text of a value (for nested documents in the grid, previews and DDL).
pub fn to_shell(b: &Bson) -> String {
    match b {
        Bson::Null | Bson::Undefined => "null".into(),
        Bson::Boolean(v) => v.to_string(),
        Bson::Int32(n) => n.to_string(),
        Bson::Int64(n) => n.to_string(),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.1}"),
        Bson::Double(f) => f.to_string(),
        Bson::String(s) => quote(s),
        Bson::ObjectId(o) => format!("ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!(
            "ISODate(\"{}\")",
            d.try_to_rfc3339_string()
                .unwrap_or_else(|_| d.timestamp_millis().to_string())
        ),
        Bson::Decimal128(d) => format!("NumberDecimal(\"{d}\")"),
        Bson::RegularExpression(r) => format!("/{}/{}", r.pattern, r.options),
        Bson::Array(a) => format!(
            "[{}]",
            a.iter().map(to_shell).collect::<Vec<_>>().join(", ")
        ),
        Bson::Document(d) => doc_to_shell(d),
        Bson::Binary(bin) if bin.subtype == BinarySubtype::Uuid && bin.bytes.len() == 16 => {
            let h: String = bin.bytes.iter().map(|x| format!("{x:02x}")).collect();
            format!(
                "UUID(\"{}-{}-{}-{}-{}\")",
                &h[0..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..]
            )
        }
        other => other.clone().into_relaxed_extjson().to_string(),
    }
}

pub fn doc_to_shell(d: &Document) -> String {
    if d.is_empty() {
        return "{}".into();
    }
    let parts: Vec<String> = d
        .iter()
        .map(|(k, v)| {
            let plain = k
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
                && k.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
            let key = if plain { k.clone() } else { quote(k) };
            format!("{key}: {}", to_shell(v))
        })
        .collect();
    format!("{{ {} }}", parts.join(", "))
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn find_with_modifiers() {
        let op = parse("db.users.find({ age: { $gt: 30 }, 'name': /^a/i }, {name: 1,}).sort({name: -1}).skip(5).limit(10);").unwrap();
        match op {
            Op::Find {
                coll,
                filter,
                projection,
                sort,
                skip,
                limit,
                one,
            } => {
                assert_eq!(coll, "users");
                assert_eq!(filter.get_document("age").unwrap(), &doc! { "$gt": 30 });
                assert!(
                    matches!(filter.get("name"), Some(Bson::RegularExpression(r)) if r.pattern == "^a" && r.options == "i")
                );
                assert_eq!(projection, Some(doc! { "name": 1 }));
                assert_eq!(sort, Some(doc! { "name": -1 }));
                assert_eq!((skip, limit, one), (Some(5), Some(10), false));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn collection_access_forms() {
        assert!(
            matches!(parse("db['my users'].find()").unwrap(), Op::Find { coll, .. } if coll == "my users")
        );
        assert!(
            matches!(parse("db.getCollection(\"a.b\").countDocuments({})").unwrap(), Op::Count { coll, .. } if coll == "a.b")
        );
        assert_eq!(parse("show collections").unwrap(), Op::ListCollections);
        assert_eq!(
            parse("db.getCollectionNames()").unwrap(),
            Op::ListCollections
        );
        assert!(matches!(
            parse("db.users.find().count()").unwrap(),
            Op::Count { .. }
        ));
    }

    #[test]
    fn helpers_and_literals() {
        let v = parse_value(
            "{ _id: ObjectId('65a1b2c3d4e5f60718293a4b'), at: ISODate('2024-02-29T13:45:01Z'), d: new Date('2024-01-01'),
               n: NumberLong(\"9007199254740993\"), i: NumberInt(7), big: 3000000000, f: 1.5, e: -2e3,
               dec: NumberDecimal('12.30'), u: UUID('a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'), s: \"a\\\"b\\n\", /* c */ ok: true, z: null, }",
        )
        .unwrap();
        let d = v.as_document().unwrap();
        assert_eq!(
            d.get_object_id("_id").unwrap().to_hex(),
            "65a1b2c3d4e5f60718293a4b"
        );
        assert_eq!(
            d.get_datetime("at").unwrap().timestamp_millis(),
            1_709_214_301_000
        );
        assert_eq!(
            d.get_datetime("d").unwrap().timestamp_millis(),
            1_704_067_200_000
        );
        assert_eq!(d.get_i64("n").unwrap(), 9_007_199_254_740_993);
        assert_eq!(d.get_i32("i").unwrap(), 7);
        assert_eq!(d.get_i64("big").unwrap(), 3_000_000_000);
        assert_eq!(d.get_f64("e").unwrap(), -2000.0);
        assert_eq!(d.get_str("s").unwrap(), "a\"b\n");
        assert!(matches!(d.get("u"), Some(Bson::Binary(b)) if b.subtype == BinarySubtype::Uuid));
        assert_eq!(to_shell(d.get("dec").unwrap()), "NumberDecimal(\"12.30\")");
        assert_eq!(
            to_shell(d.get("u").unwrap()),
            "UUID(\"a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11\")"
        );
    }

    #[test]
    fn writes_and_read_only_classification() {
        assert!(parse("db.u.insertOne({a: 1})").unwrap().is_write());
        assert!(
            matches!(parse("db.u.insertMany([{a: 1}, {a: 2}])").unwrap(), Op::InsertMany { docs, .. } if docs.len() == 2)
        );
        assert!(matches!(
            parse("db.u.updateMany({}, {$set: {a: 1}}, {upsert: true})").unwrap(),
            Op::Update {
                many: true,
                upsert: true,
                ..
            }
        ));
        assert!(matches!(
            parse("db.u.updateOne({}, [{$set: {a: 1}}])").unwrap(),
            Op::Update {
                update: UpdateSpec::Pipeline(_),
                ..
            }
        ));
        assert!(!parse("db.u.aggregate([{$match: {}}])").unwrap().is_write());
        assert!(parse("db.u.aggregate([{$match: {}}, {$out: 'x'}])")
            .unwrap()
            .is_write());
        assert!(!parse("db.runCommand({ping: 1})").unwrap().is_write());
        assert!(parse("db.runCommand({dropDatabase: 1})")
            .unwrap()
            .is_write());
        assert!(!parse("db.u.find()").unwrap().is_write());
    }

    #[test]
    fn errors_are_readable() {
        assert!(parse("db.u.find({a: })")
            .unwrap_err()
            .contains("expected a value"));
        assert!(parse("db.u.frobnicate()")
            .unwrap_err()
            .contains("unsupported method"));
        assert!(parse("select 1").unwrap_err().contains("expected `db`"));
        assert!(parse("db.u.find() extra")
            .unwrap_err()
            .contains("unexpected text"));
    }

    #[test]
    fn shell_roundtrip() {
        let d = doc! { "name": "ann", "tags": ["a", "b"], "n": 1.0, "weird key": { "x": true } };
        let text = doc_to_shell(&d);
        assert_eq!(
            text,
            "{ name: \"ann\", tags: [\"a\", \"b\"], n: 1.0, \"weird key\": { x: true } }"
        );
        assert_eq!(parse_document(&text).unwrap(), d);
    }
}
