//! MongoDB driver, on the official `mongodb` crate (sync API). Written from the MongoDB
//! manual (CRUD, aggregation, indexes, database commands, shell methods).
//!
//! The query editor takes shell-style commands (see [`shell`]). Tables are collections:
//! the engine's table view pages through them with [`Session::fetch_page`], where the
//! filter box takes a query document such as `{ age: { $gt: 30 } }`.

pub mod shell;

/// Re-exported so clients can build and read documents with the same BSON types.
pub use mongodb::bson;

use mongodb::bson::{doc, oid::ObjectId, Binary, Bson, Document};
use mongodb::sync::{Client, Database};
use mongodb::IndexModel;
use shell::{doc_to_shell, to_shell, Op, UpdateSpec};
use std::time::Duration;
use thaan_core::connection::ConnectionConfig;
use thaan_core::dialect::Dialect;
use thaan_core::driver::{Driver, DriverInfo, ParamKind, ParamSpec, Session};
use thaan_core::schema::{ColumnInfo, IndexInfo, ObjectKind, ObjectRef, SchemaObject, TableInfo};
use thaan_core::table::{Filter, FilterOp, PageRequest, StagedChange};
use thaan_core::{Column, Error, Result, ResultSet, Value};

/// `find` without `.limit()` returns at most this many documents to the grid.
pub const DEFAULT_FIND_LIMIT: i64 = 1000;
/// Documents sampled to infer a collection's fields.
const SAMPLE_SIZE: i32 = 100;

pub struct MongoDriver;

impl Driver for MongoDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mongodb".into(),
            name: "MongoDB".into(),
            params: vec![
                ParamSpec::new("url", "Connection string", ParamKind::Text)
                    .default("mongodb://localhost:27017")
                    .placeholder("mongodb://host:27017/?replicaSet=rs0"),
                ParamSpec::new("user", "User", ParamKind::Text).placeholder("(optional)"),
                ParamSpec::new("password", "Password", ParamKind::Password),
                ParamSpec::new("database", "Database", ParamKind::Text).placeholder("test"),
            ],
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::MONGODB
    }

    fn connect(&self, c: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let uri = build_uri(c)?;
        let mut opts = mongodb::options::ClientOptions::parse(&uri)
            .run()
            .map_err(db)?;
        opts.app_name = Some("thaan".into());
        opts.connect_timeout.get_or_insert(Duration::from_secs(10));
        opts.server_selection_timeout
            .get_or_insert(Duration::from_secs(10));
        let client = Client::with_options(opts).map_err(db)?;
        let name = c
            .param("database")
            .map(str::to_string)
            .or_else(|| client.default_database().map(|d| d.name().to_string()))
            .unwrap_or_else(|| "test".into());
        let mut s = MongoSession {
            db: client.database(&name),
            client,
            read_only: c.read_only,
        };
        s.ping()?;
        Ok(Box::new(s))
    }
}

/// Puts the (separately stored) credentials back into the connection string.
fn build_uri(c: &ConnectionConfig) -> Result<String> {
    let url = c.param("url").unwrap_or("mongodb://localhost:27017");
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Error::InvalidParams(format!("not a MongoDB connection string: {url}")))?;
    let mut uri = format!("{scheme}://");
    if let Some(user) = c.param("user") {
        uri.push_str(&encode(user));
        if let Some(pw) = c.param("password") {
            uri.push(':');
            uri.push_str(&encode(pw));
        }
        uri.push('@');
    }
    uri.push_str(rest);
    Ok(uri)
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The server's own sentence when there is one (e.g. `E11000 duplicate key error …`),
/// instead of the driver's debug dump.
fn db(e: mongodb::error::Error) -> Error {
    use mongodb::error::{ErrorKind, WriteFailure};
    let msg = match e.kind.as_ref() {
        ErrorKind::Command(c) => Some(c.message.clone()),
        ErrorKind::Write(WriteFailure::WriteError(w)) => Some(w.message.clone()),
        ErrorKind::Write(WriteFailure::WriteConcernError(w)) => Some(w.message.clone()),
        ErrorKind::InsertMany(m) => m
            .write_errors
            .as_ref()
            .and_then(|v| v.first())
            .map(|w| w.message.clone()),
        _ => None,
    };
    Error::Database(msg.unwrap_or_else(|| e.to_string()))
}

pub struct MongoSession {
    client: Client,
    db: Database,
    read_only: bool,
}

// ---- value conversion -------------------------------------------------------------------

fn type_label(b: &Bson) -> &'static str {
    match b {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null | Bson::Undefined => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::Timestamp(_) => "timestamp",
        Bson::Binary(_) => "binData",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Decimal128(_) => "decimal",
        _ => "other",
    }
}

/// A grid cell. Scalars map to scalars; ObjectIds show their hex; nested values show as
/// shell text (`{ a: 1 }`), which the editor parses back.
pub fn to_value(b: &Bson) -> Value {
    match b {
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Boolean(v) => Value::Bool(*v),
        Bson::Int32(n) => Value::Int(*n as i64),
        Bson::Int64(n) => Value::Int(*n),
        Bson::Double(f) => Value::Float(*f),
        Bson::String(s) => Value::Text(s.clone()),
        Bson::ObjectId(o) => Value::Text(o.to_hex()),
        Bson::DateTime(d) => Value::Text(
            d.try_to_rfc3339_string()
                .unwrap_or_else(|_| d.timestamp_millis().to_string()),
        ),
        Bson::Decimal128(d) => Value::Text(d.to_string()),
        Bson::Binary(bin) if bin.subtype != bson::spec::BinarySubtype::Uuid => {
            Value::Bytes(bin.bytes.clone())
        }
        other => Value::Text(to_shell(other)),
    }
}

/// The reverse of [`to_value`] for edited cells and filter values.
pub fn to_bson(column: &str, v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Int(i) => i32::try_from(*i)
            .map(Bson::Int32)
            .unwrap_or(Bson::Int64(*i)),
        Value::Float(f) => Bson::Double(*f),
        Value::Bytes(b) => Bson::Binary(Binary {
            subtype: bson::spec::BinarySubtype::Generic,
            bytes: b.clone(),
        }),
        Value::Text(s) => {
            let t = s.trim();
            if column == "_id" && t.len() == 24 {
                if let Ok(o) = ObjectId::parse_str(t) {
                    return Bson::ObjectId(o);
                }
            }
            let looks_structured = t.starts_with('{')
                || t.starts_with('[')
                || [
                    "ObjectId(",
                    "ISODate(",
                    "NumberDecimal(",
                    "NumberLong(",
                    "UUID(",
                ]
                .iter()
                .any(|h| t.starts_with(h));
            if looks_structured {
                if let Ok(b) = shell::parse_value(t) {
                    return b;
                }
            }
            Bson::String(s.clone())
        }
    }
}

/// Documents → rows. Columns are the union of top-level fields in first-seen order,
/// with `_id` first.
pub fn docs_to_result(docs: &[Document]) -> ResultSet {
    let mut names: Vec<String> = Vec::new();
    if docs.iter().any(|d| d.contains_key("_id")) {
        names.push("_id".into());
    }
    for d in docs {
        for k in d.keys() {
            if !names.iter().any(|n| n == k) {
                names.push(k.clone());
            }
        }
    }
    let columns = names
        .iter()
        .map(|n| Column {
            name: n.clone(),
            type_name: docs
                .iter()
                .filter_map(|d| d.get(n))
                .find(|b| !matches!(b, Bson::Null))
                .map(|b| type_label(b).to_string()),
        })
        .collect();
    let rows = docs
        .iter()
        .map(|d| {
            names
                .iter()
                .map(|n| d.get(n).map(to_value).unwrap_or(Value::Null))
                .collect()
        })
        .collect();
    ResultSet {
        columns,
        rows,
        documents: Some(
            docs.iter()
                .map(|d| to_doc(&Bson::Document(d.clone())))
                .collect(),
        ),
        ..Default::default()
    }
}

/// BSON → the engine's typed document tree (for the tree and JSON views).
pub fn to_doc(b: &Bson) -> thaan_core::doc::Doc {
    use thaan_core::doc::Doc;
    match b {
        Bson::Null | Bson::Undefined => Doc::Null,
        Bson::Boolean(v) => Doc::Bool(*v),
        Bson::Int32(n) => Doc::Int32(*n),
        Bson::Int64(n) => Doc::Int64(*n),
        Bson::Double(f) => Doc::Double(*f),
        Bson::Decimal128(d) => Doc::Decimal(d.to_string()),
        Bson::String(s) => Doc::String(s.clone()),
        Bson::ObjectId(o) => Doc::ObjectId(o.to_hex()),
        Bson::DateTime(d) => Doc::Date(
            d.try_to_rfc3339_string()
                .unwrap_or_else(|_| d.timestamp_millis().to_string()),
        ),
        Bson::Binary(_) => Doc::Binary(to_shell(b)),
        Bson::RegularExpression(_) => Doc::Regex(to_shell(b)),
        Bson::Array(a) => Doc::Array(a.iter().map(to_doc).collect()),
        Bson::Document(d) => Doc::Object(d.iter().map(|(k, v)| (k.clone(), to_doc(v))).collect()),
        other => Doc::Other {
            type_name: type_label(other).to_string(),
            text: to_shell(other),
        },
    }
}

fn single(column: &str, rows: Vec<Value>) -> ResultSet {
    ResultSet {
        columns: vec![Column::new(column)],
        rows: rows.into_iter().map(|v| vec![v]).collect(),
        ..Default::default()
    }
}

fn affected(n: u64) -> ResultSet {
    ResultSet {
        rows_affected: Some(n),
        ..Default::default()
    }
}

// ---- table-view filters -----------------------------------------------------------------

/// SQL `LIKE` pattern → anchored regex.
fn like_to_regex(p: &str) -> String {
    let mut out = String::from("^");
    for c in p.chars() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            c if "\\.^$|?*+()[]{}".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

/// Structured filters plus the raw filter box (a query document), combined with `$and`.
pub fn filter_document(
    filters: &[Filter],
    raw: Option<&str>,
) -> std::result::Result<Document, String> {
    let mut parts: Vec<Document> = Vec::new();
    if let Some(r) = raw.map(str::trim).filter(|r| !r.is_empty()) {
        parts.push(shell::parse_document(r)?);
    }
    for f in filters {
        let v = f
            .values
            .first()
            .map(|v| to_bson(&f.column, v))
            .unwrap_or(Bson::Null);
        let cond = match f.op {
            FilterOp::Eq => doc! { "$eq": v },
            FilterOp::Ne => doc! { "$ne": v },
            FilterOp::Lt => doc! { "$lt": v },
            FilterOp::Le => doc! { "$lte": v },
            FilterOp::Gt => doc! { "$gt": v },
            FilterOp::Ge => doc! { "$gte": v },
            FilterOp::IsNull => doc! { "$eq": Bson::Null },
            FilterOp::IsNotNull => doc! { "$ne": Bson::Null },
            FilterOp::In => {
                doc! { "$in": f.values.iter().map(|x| to_bson(&f.column, x)).collect::<Vec<_>>() }
            }
            FilterOp::Like | FilterOp::NotLike => {
                let re = Bson::RegularExpression(bson::Regex {
                    pattern: like_to_regex(
                        &v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| to_shell(&v)),
                    ),
                    options: String::new(),
                });
                if f.op == FilterOp::Like {
                    doc! { "$regex": re }
                } else {
                    doc! { "$not": re }
                }
            }
        };
        parts.push(doc! { f.column.clone(): cond });
    }
    Ok(match parts.len() {
        0 => Document::new(),
        1 => parts.remove(0),
        _ => doc! { "$and": parts },
    })
}

fn key_filter(key: &[(String, Value)]) -> Document {
    // Never let an empty key match every document.
    if key.is_empty() {
        return doc! { "_id": { "$exists": false, "$eq": "__never__" } };
    }
    key.iter()
        .map(|(c, v)| (c.clone(), to_bson(c, v)))
        .collect()
}

fn change_ops(table: &str, changes: &[StagedChange]) -> Vec<Op> {
    changes
        .iter()
        .map(|c| match c {
            StagedChange::Insert { values } => Op::InsertOne {
                coll: table.into(),
                doc: values.iter().map(|(k, v)| (k.clone(), to_bson(k, v))).collect(),
            },
            StagedChange::Update { key, set } => Op::Update {
                coll: table.into(),
                filter: key_filter(key),
                update: UpdateSpec::Doc(doc! { "$set": set.iter().map(|(k, v)| (k.clone(), to_bson(k, v))).collect::<Document>() }),
                many: false,
                upsert: false,
            },
            StagedChange::Delete { key } => Op::Delete { coll: table.into(), filter: key_filter(key), many: false },
        })
        .collect()
}

/// Shell text of an operation, for previews.
pub fn op_to_shell(op: &Op) -> String {
    let c = |coll: &str| {
        if coll.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
            && !coll.starts_with(|ch: char| ch.is_ascii_digit())
        {
            format!("db.{coll}")
        } else {
            format!("db.getCollection({})", to_shell(&Bson::String(coll.into())))
        }
    };
    match op {
        Op::InsertOne { coll, doc } => format!("{}.insertOne({})", c(coll), doc_to_shell(doc)),
        Op::Update {
            coll,
            filter,
            update: UpdateSpec::Doc(u),
            many,
            ..
        } => {
            format!(
                "{}.{}({}, {})",
                c(coll),
                if *many { "updateMany" } else { "updateOne" },
                doc_to_shell(filter),
                doc_to_shell(u)
            )
        }
        Op::Delete { coll, filter, many } => {
            format!(
                "{}.{}({})",
                c(coll),
                if *many { "deleteMany" } else { "deleteOne" },
                doc_to_shell(filter)
            )
        }
        other => format!("{other:?}"),
    }
}

// ---- execution ----------------------------------------------------------------------------

impl MongoSession {
    fn run(&mut self, op: Op) -> Result<ResultSet> {
        if self.read_only && op.is_write() {
            return Err(Error::ReadOnly(op.name().to_string()));
        }
        let coll = |name: &str| self.db.collection::<Document>(name);
        let rs = match op {
            Op::Find {
                coll: name,
                filter,
                projection,
                sort,
                skip,
                limit,
                ..
            } => {
                let c = coll(&name);
                let mut f = c.find(filter).limit(limit.unwrap_or(DEFAULT_FIND_LIMIT));
                if let Some(p) = projection {
                    f = f.projection(p);
                }
                if let Some(s) = sort {
                    f = f.sort(s);
                }
                if let Some(s) = skip {
                    f = f.skip(s);
                }
                let docs: Vec<Document> = f
                    .run()
                    .map_err(db)?
                    .collect::<std::result::Result<_, _>>()
                    .map_err(db)?;
                docs_to_result(&docs)
            }
            Op::Count { coll: name, filter } => {
                let n = coll(&name).count_documents(filter).run().map_err(db)?;
                single("count", vec![Value::Int(n as i64)])
            }
            Op::Distinct {
                coll: name,
                field,
                filter,
            } => {
                let vals = coll(&name).distinct(&field, filter).run().map_err(db)?;
                single(&field, vals.iter().map(to_value).collect())
            }
            Op::Aggregate {
                coll: name,
                pipeline,
            } => {
                let docs: Vec<Document> = coll(&name)
                    .aggregate(pipeline)
                    .run()
                    .map_err(db)?
                    .collect::<std::result::Result<_, _>>()
                    .map_err(db)?;
                docs_to_result(&docs)
            }
            Op::InsertOne { coll: name, doc } => {
                coll(&name).insert_one(doc).run().map_err(db)?;
                affected(1)
            }
            Op::InsertMany { coll: name, docs } => {
                let r = coll(&name).insert_many(docs).run().map_err(db)?;
                affected(r.inserted_ids.len() as u64)
            }
            Op::Update {
                coll: name,
                filter,
                update,
                many,
                upsert,
            } => {
                let update: mongodb::options::UpdateModifications = match update {
                    UpdateSpec::Doc(d) => d.into(),
                    UpdateSpec::Pipeline(p) => p.into(),
                };
                let r = if many {
                    coll(&name).update_many(filter, update).upsert(upsert).run()
                } else {
                    coll(&name).update_one(filter, update).upsert(upsert).run()
                }
                .map_err(db)?;
                affected(r.modified_count + r.upserted_id.map(|_| 1).unwrap_or(0))
            }
            Op::Replace {
                coll: name,
                filter,
                doc,
                upsert,
            } => {
                let r = coll(&name)
                    .replace_one(filter, doc)
                    .upsert(upsert)
                    .run()
                    .map_err(db)?;
                affected(r.modified_count + r.upserted_id.map(|_| 1).unwrap_or(0))
            }
            Op::Delete {
                coll: name,
                filter,
                many,
            } => {
                let r = if many {
                    coll(&name).delete_many(filter).run()
                } else {
                    coll(&name).delete_one(filter).run()
                }
                .map_err(db)?;
                affected(r.deleted_count)
            }
            Op::CreateIndex {
                coll: name,
                keys,
                options,
            } => {
                let opts: mongodb::options::IndexOptions = bson::from_document(options)
                    .map_err(|e| Error::InvalidParams(format!("index options: {e}")))?;
                let r = coll(&name)
                    .create_index(IndexModel::builder().keys(keys).options(opts).build())
                    .run()
                    .map_err(db)?;
                single("index", vec![Value::Text(r.index_name)])
            }
            Op::GetIndexes { coll: name } => {
                let docs: Vec<Document> = coll(&name)
                    .list_indexes()
                    .run()
                    .map_err(db)?
                    .map(|r| {
                        r.map(|ix| {
                            let mut d = doc! { "key": ix.keys.clone() };
                            if let Some(o) = ix.options {
                                if let Some(n) = o.name {
                                    d.insert("name", n);
                                }
                                d.insert("unique", o.unique.unwrap_or(false));
                            }
                            d
                        })
                    })
                    .collect::<std::result::Result<_, _>>()
                    .map_err(db)?;
                docs_to_result(&docs)
            }
            Op::DropCollection { coll: name } => {
                coll(&name).drop().run().map_err(db)?;
                affected(0)
            }
            Op::RenameCollection {
                coll: name,
                to,
                drop_target,
            } => {
                // renameCollection is an admin command taking full `db.collection` names.
                let db_name = self.db.name().to_string();
                self.client
                    .database("admin")
                    .run_command(bson::doc! {
                        "renameCollection": format!("{db_name}.{name}"),
                        "to": format!("{db_name}.{to}"),
                        "dropTarget": drop_target,
                    })
                    .run()
                    .map_err(db)?;
                affected(0)
            }
            Op::CreateCollection { name, options } => {
                let opts: mongodb::options::CreateCollectionOptions = bson::from_document(options)
                    .map_err(|e| Error::InvalidParams(format!("collection options: {e}")))?;
                self.db
                    .create_collection(&name)
                    .with_options(opts)
                    .run()
                    .map_err(db)?;
                affected(0)
            }
            Op::ListCollections => {
                let mut names = self.db.list_collection_names().run().map_err(db)?;
                names.sort();
                single("collection", names.into_iter().map(Value::Text).collect())
            }
            Op::ListDatabases => {
                let names = self.client.list_database_names().run().map_err(db)?;
                single("database", names.into_iter().map(Value::Text).collect())
            }
            Op::RunCommand { command } => {
                let d = self.db.run_command(command).run().map_err(db)?;
                docs_to_result(&[d])
            }
        };
        Ok(rs)
    }

    fn sample(&mut self, coll: &str) -> Result<Vec<Document>> {
        self.db
            .collection::<Document>(coll)
            .aggregate([doc! { "$sample": { "size": SAMPLE_SIZE } }])
            .run()
            .map_err(db)?
            .collect::<std::result::Result<_, _>>()
            .map_err(db)
    }

    fn indexes(&mut self, coll: &str) -> Result<Vec<IndexModel>> {
        self.db
            .collection::<Document>(coll)
            .list_indexes()
            .run()
            .map_err(db)?
            .collect::<std::result::Result<_, _>>()
            .map_err(db)
    }
}

impl Session for MongoSession {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<ResultSet> {
        let op = shell::parse(sql).map_err(Error::InvalidParams)?;
        self.run(op)
    }

    fn ping(&mut self) -> Result<()> {
        self.db
            .run_command(doc! { "ping": 1 })
            .run()
            .map(|_| ())
            .map_err(db)
    }

    // MongoDB transactions need a replica set; staged edits are applied one by one instead.
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
        let mut names = self.client.list_database_names().run().map_err(db)?;
        names.sort();
        Ok(names)
    }

    fn current_database(&mut self) -> Result<Option<String>> {
        Ok(Some(self.db.name().to_string()))
    }

    fn list_objects(&mut self) -> Result<Vec<SchemaObject>> {
        let mut out = Vec::new();
        let mut specs: Vec<_> = self
            .db
            .list_collections()
            .run()
            .map_err(db)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db)?;
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        for spec in specs {
            if spec.name.starts_with("system.") {
                continue;
            }
            let is_view = matches!(spec.collection_type, mongodb::results::CollectionType::View);
            out.push(SchemaObject {
                object: ObjectRef::new(spec.name.clone()),
                kind: if is_view {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                },
                parent: None,
            });
            if !is_view {
                for ix in self.indexes(&spec.name)? {
                    if let Some(name) = ix.options.and_then(|o| o.name).filter(|n| n != "_id_") {
                        out.push(SchemaObject {
                            object: ObjectRef::new(name),
                            kind: ObjectKind::Index,
                            parent: Some(spec.name.clone()),
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Fields are inferred from a sample of documents: the type column lists every type
    /// seen, and a field is "nullable" when some sampled document lacks it or holds null.
    fn describe_table(&mut self, t: &ObjectRef) -> Result<TableInfo> {
        let docs = self.sample(&t.name)?;
        let mut fields: Vec<(String, Vec<&'static str>, usize)> = Vec::new();
        for d in &docs {
            for (k, v) in d {
                let label = type_label(v);
                match fields.iter_mut().find(|(n, _, _)| n == k) {
                    Some((_, types, count)) => {
                        if !types.contains(&label) {
                            types.push(label);
                        }
                        *count += 1;
                    }
                    None => fields.push((k.clone(), vec![label], 1)),
                }
            }
        }
        fields.sort_by_key(|(n, _, _)| n != "_id");
        let columns = fields
            .into_iter()
            .map(|(name, types, count)| ColumnInfo {
                primary_key: (name == "_id").then_some(1),
                nullable: count < docs.len() || types.contains(&"null"),
                data_type: types.join(" | "),
                default: None,
                comment: None,
                auto_increment: false,
                name,
            })
            .collect();
        let indexes = self
            .indexes(&t.name)?
            .into_iter()
            .map(|ix| IndexInfo {
                primary: ix.keys.keys().eq(["_id"].iter()),
                unique: ix.options.as_ref().and_then(|o| o.unique).unwrap_or(false)
                    || ix.keys.keys().eq(["_id"].iter()),
                name: ix.options.and_then(|o| o.name).unwrap_or_default(),
                columns: ix.keys.keys().cloned().collect(),
            })
            .collect();
        Ok(TableInfo {
            object: t.clone(),
            columns,
            foreign_keys: vec![],
            indexes,
            triggers: vec![],
        })
    }

    fn ddl(&mut self, o: &ObjectRef) -> Result<String> {
        let spec = self
            .db
            .list_collections()
            .filter(doc! { "name": &o.name })
            .run()
            .map_err(db)?
            .next()
            .transpose()
            .map_err(db)?
            .ok_or_else(|| Error::Database(format!("no such collection: {}", o.name)))?;
        let name = to_shell(&Bson::String(o.name.clone()));
        let opts = bson::to_document(&spec.options).unwrap_or_default();
        let opts: Document = opts
            .into_iter()
            .filter(|(_, v)| !matches!(v, Bson::Null))
            .collect();
        let mut out = if opts.is_empty() {
            format!("db.createCollection({name});")
        } else {
            format!("db.createCollection({name}, {});", doc_to_shell(&opts))
        };
        if !matches!(spec.collection_type, mongodb::results::CollectionType::View) {
            for ix in self.indexes(&o.name)? {
                let n = ix
                    .options
                    .as_ref()
                    .and_then(|x| x.name.clone())
                    .unwrap_or_default();
                if n == "_id_" {
                    continue;
                }
                let mut io = doc! { "name": n };
                if ix.options.as_ref().and_then(|x| x.unique) == Some(true) {
                    io.insert("unique", true);
                }
                out.push_str(&format!(
                    "\ndb.getCollection({name}).createIndex({}, {});",
                    doc_to_shell(&ix.keys),
                    doc_to_shell(&io)
                ));
            }
        }
        Ok(out)
    }

    fn fetch_page(&mut self, req: &PageRequest, paged: bool) -> Option<Result<(ResultSet, u64)>> {
        Some((|| {
            let filter = filter_document(&req.filters, req.raw_where.as_deref())
                .map_err(Error::InvalidParams)?;
            let coll = self.db.collection::<Document>(&req.table.name);
            let mut find = coll.find(filter.clone());
            if let Some(p) = req.projection.as_deref().filter(|p| !p.trim().is_empty()) {
                find = find.projection(
                    shell::parse_document(p)
                        .map_err(|e| Error::InvalidParams(format!("projection: {e}")))?,
                );
            }
            if let Some(srt) = req.raw_sort.as_deref().filter(|p| !p.trim().is_empty()) {
                find = find.sort(
                    shell::parse_document(srt)
                        .map_err(|e| Error::InvalidParams(format!("sort: {e}")))?,
                );
            } else if !req.sort.is_empty() {
                find = find.sort(
                    req.sort
                        .iter()
                        .map(|s| {
                            (
                                s.column.clone(),
                                Bson::Int32(if s.descending { -1 } else { 1 }),
                            )
                        })
                        .collect::<Document>(),
                );
            }
            if paged {
                let size = req.page_size.max(1);
                find = find.skip(req.page * size).limit(size as i64);
            }
            let docs: Vec<Document> = find
                .run()
                .map_err(db)?
                .collect::<std::result::Result<_, _>>()
                .map_err(db)?;
            let total = if paged {
                coll.count_documents(filter).run().map_err(db)?
            } else {
                docs.len() as u64
            };
            let mut rs = docs_to_result(&docs);
            rs.sql = format!(
                "db.getCollection({}).find({})",
                to_shell(&Bson::String(req.table.name.clone())),
                req.raw_where.as_deref().unwrap_or("{}")
            );
            Ok((rs, total))
        })())
    }

    fn preview_changes(&mut self, t: &ObjectRef, changes: &[StagedChange]) -> Option<Vec<String>> {
        Some(
            change_ops(&t.name, changes)
                .iter()
                .map(op_to_shell)
                .collect(),
        )
    }

    fn apply_changes(&mut self, t: &ObjectRef, changes: &[StagedChange]) -> Option<Result<u64>> {
        Some((|| {
            let mut n = 0;
            for op in change_ops(&t.name, changes) {
                n += self.run(op)?.rows_affected.unwrap_or(0);
            }
            Ok(n)
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use thaan_core::engine::{Engine, QueryRequest, RunScope};
    use thaan_core::store::Store;

    #[test]
    fn uri_gets_credentials_back() {
        let c =
            ConnectionConfig::from_url("mongodb://ad%3Amin:p@ss@h:1/app?authSource=admin").unwrap();
        assert_eq!(
            build_uri(&c).unwrap(),
            "mongodb://ad%3Amin:p%40ss@h:1/app?authSource=admin"
        );
    }

    #[test]
    fn grid_conversion() {
        let oid = ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap();
        let rs = docs_to_result(&[
            doc! { "name": "a", "_id": oid, "tags": ["x"] },
            doc! { "_id": 2, "age": 3 },
        ]);
        assert_eq!(
            rs.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["_id", "name", "tags", "age"]
        );
        assert_eq!(
            rs.rows[0],
            vec![
                Value::from("65a1b2c3d4e5f60718293a4b"),
                Value::from("a"),
                Value::from("[\"x\"]"),
                Value::Null
            ]
        );
        assert_eq!(
            to_bson("_id", &Value::from("65a1b2c3d4e5f60718293a4b")),
            Bson::ObjectId(oid)
        );
        assert_eq!(
            to_bson("tags", &Value::from("[\"x\"]")),
            Bson::Array(vec![Bson::String("x".into())])
        );
        assert_eq!(
            to_bson("name", &Value::from("65a1b2c3d4e5f60718293a4b")),
            Bson::String("65a1b2c3d4e5f60718293a4b".into())
        );
    }

    #[test]
    fn table_filters() {
        let f = filter_document(
            &[
                Filter {
                    column: "age".into(),
                    op: FilterOp::Ge,
                    values: vec![Value::Int(18)],
                },
                Filter {
                    column: "name".into(),
                    op: FilterOp::Like,
                    values: vec![Value::from("a%.")],
                },
            ],
            Some("{ country: 'TH' }"),
        )
        .unwrap();
        assert_eq!(
            doc_to_shell(&f),
            "{ $and: [{ country: \"TH\" }, { age: { $gte: 18 } }, { name: { $regex: /^a.*\\.$/ } }] }"
        );
        assert!(filter_document(&[], Some("{ broken")).is_err());
        assert_eq!(filter_document(&[], Some("  ")).unwrap(), Document::new());
    }

    #[test]
    fn previews() {
        let p = change_ops(
            "users",
            &[StagedChange::Update {
                key: vec![("_id".into(), Value::from("65a1b2c3d4e5f60718293a4b"))],
                set: vec![("age".into(), Value::Int(3))],
            }],
        );
        assert_eq!(op_to_shell(&p[0]), "db.users.updateOne({ _id: ObjectId(\"65a1b2c3d4e5f60718293a4b\") }, { $set: { age: 3 } })");
        let d = change_ops("my users", &[StagedChange::Delete { key: vec![] }]);
        assert!(op_to_shell(&d[0]).starts_with("db.getCollection(\"my users\").deleteOne("));
    }

    fn shaped_request(t: &ObjectRef) -> PageRequest {
        PageRequest {
            table: t.clone(),
            filters: vec![],
            raw_where: None,
            projection: None,
            raw_sort: None,
            sort: vec![],
            page: 0,
            page_size: 10,
        }
    }

    #[test]
    #[ignore = "needs THAAN_TEST_MONGODB_URL"]
    fn live_roundtrip() {
        let Ok(url) = std::env::var("THAAN_TEST_MONGODB_URL") else {
            return;
        };
        let e = Engine::new(Store::in_memory());
        e.register_driver(Arc::new(MongoDriver));
        let mut cfg = ConnectionConfig::from_url(&url).unwrap();
        cfg.params.insert("database".into(), "thaan_test".into());
        let sid = e.open(cfg.clone()).unwrap().session_id;
        let run = |sql: &str| {
            e.run_query(&QueryRequest {
                session_id: sid,
                sql: sql.into(),
                scope: RunScope::All,
                params: Default::default(),
            })
            .unwrap()
        };

        let r = run(
            "db.users.drop(); db.users.insertMany([{name: 'ann', age: 31, country: 'TH', at: ISODate('2024-02-29T13:45:01Z')},
             {name: 'bob', age: 17, country: 'JP', tags: ['x', 'y']}, {name: 'cy', age: 45, country: 'TH', addr: {city: 'BKK'}}]);
             db.users.createIndex({name: 1}, {unique: true, name: 'by_name'})",
        );
        assert!(r.iter().all(|x| x.error.is_none()), "{r:?}");
        assert_eq!(r[1].result.as_ref().unwrap().rows_affected, Some(3));

        let r =
            run("db.users.find({country: 'TH'}, {_id: 0, name: 1, age: 1, at: 1}).sort({age: -1})");
        let rs = r[0].result.as_ref().unwrap();
        assert_eq!(
            rs.rows,
            vec![
                vec![Value::from("cy"), Value::Int(45), Value::Null],
                vec![
                    Value::from("ann"),
                    Value::Int(31),
                    Value::from("2024-02-29T13:45:01Z")
                ],
            ]
        );
        let r = run(
            "db.users.aggregate([{$group: {_id: '$country', n: {$sum: 1}}}, {$sort: {_id: 1}}])",
        );
        assert_eq!(
            r[0].result.as_ref().unwrap().rows,
            vec![
                vec![Value::from("JP"), Value::Int(1)],
                vec![Value::from("TH"), Value::Int(2)]
            ]
        );
        let r = run("db.users.updateOne({name: 'bob'}, {$inc: {age: 1}}); db.users.countDocuments({age: {$gte: 18}})");
        assert_eq!(r[0].result.as_ref().unwrap().rows_affected, Some(1));
        assert_eq!(
            r[1].result.as_ref().unwrap().rows,
            vec![vec![Value::Int(3)]]
        );

        let objs = e.list_objects(sid).unwrap();
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::Table && o.object.name == "users"));
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::Index && o.object.name == "by_name"));
        let t = ObjectRef::new("users");
        let info = e.describe_table(sid, &t).unwrap();
        assert_eq!(info.primary_key(), ["_id"]);
        assert!(info
            .columns
            .iter()
            .any(|c| c.name == "tags" && c.nullable && c.data_type == "array"));
        assert!(e
            .ddl(sid, &t)
            .unwrap()
            .contains("createIndex({ name: 1 }, { name: \"by_name\", unique: true })"));

        let page = e
            .fetch_page(
                sid,
                &PageRequest {
                    table: t.clone(),
                    filters: vec![],
                    raw_where: Some("{country: 'TH'}".into()),
                    projection: None,
                    raw_sort: None,
                    sort: vec![thaan_core::table::Sort {
                        column: "age".into(),
                        descending: false,
                    }],
                    page: 0,
                    page_size: 1,
                },
            )
            .unwrap();
        assert_eq!(page.total_rows, 2);
        assert_eq!(
            page.result.rows[0][page.result.column_index("name").unwrap()],
            Value::from("ann")
        );
        // Typed documents ride along for the tree / JSON views.
        let docs = page.result.documents.as_ref().unwrap();
        assert_eq!(docs[0].get("at").unwrap().type_name(), "Date");

        // Projection and a sort document typed in the collection view.
        let shaped = e
            .fetch_page(
                sid,
                &PageRequest {
                    table: t.clone(),
                    filters: vec![],
                    raw_where: None,
                    projection: Some("{ _id: 0, name: 1, age: 1 }".into()),
                    raw_sort: Some("{ age: -1 }".into()),
                    sort: vec![],
                    page: 0,
                    page_size: 10,
                },
            )
            .unwrap();
        assert_eq!(
            shaped
                .result
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["name", "age"]
        );
        assert_eq!(
            shaped.result.rows[0],
            vec![Value::from("cy"), Value::Int(45)]
        );
        let bad = e.fetch_page(
            sid,
            &PageRequest {
                projection: Some("{ broken".into()),
                ..shaped_request(&t)
            },
        );
        assert!(bad.unwrap_err().to_string().contains("projection"));

        let id = page.result.rows[0][0].clone();
        let n = e
            .apply_changes(
                sid,
                &t,
                &[StagedChange::Update {
                    key: vec![("_id".into(), id)],
                    set: vec![("age".into(), Value::Int(32))],
                }],
            )
            .unwrap();
        assert_eq!(n, 1);
        let r = run("db.users.findOne({name: 'ann'})");
        let rs = r[0].result.as_ref().unwrap();
        assert_eq!(rs.rows[0][rs.column_index("age").unwrap()], Value::Int(32));

        // Read-only connections refuse writes in the driver.
        cfg.read_only = true;
        let ro = e.open(cfg).unwrap().session_id;
        let r = e
            .run_query(&QueryRequest {
                session_id: ro,
                sql: "db.users.deleteMany({})".into(),
                scope: RunScope::All,
                params: Default::default(),
            })
            .unwrap();
        assert!(r[0].error.as_ref().unwrap().contains("read-only"), "{r:?}");
        run("db.users.drop()");
    }
}
