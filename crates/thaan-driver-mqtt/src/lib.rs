//! MQTT driver: a broker as a browsable message store. Written from the MQTT 3.1.1
//! standard (OASIS). Pure Rust, `std::net` only.
//!
//! Model:
//! - On connect the session subscribes to the connection's topic filter (`#` by default)
//!   and a background thread keeps every message it receives in a ring buffer of the last
//!   [`DEFAULT_BUFFER`] messages (retained messages arrive first).
//! - Objects in the tree are the topics seen so far, plus the filter itself (`#`), which
//!   shows every buffered message. Opening a topic shows its messages newest first:
//!   `received_at | topic | payload | qos | retain | bytes`.
//! - The query editor takes one command per line:
//!   `PUBLISH <topic> <payload> [QOS 0|1] [RETAIN]`, `SUBSCRIBE <filter> [QOS 0|1]`,
//!   `UNSUBSCRIBE <filter>`, `MESSAGES [filter] [limit]`, `TOPICS`, `CLEAR`.
//! - Adding a row to a topic publishes it; deleting a retained row clears the retained
//!   message (an empty retained publish). Read-only connections refuse both.

pub mod packet;

use packet::{topic_matches, Incoming};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufReader, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thaan_core::browse::{BrokerMessage, Subscription};
use thaan_core::cmdline::{quote_arg, split_args};
use thaan_core::connection::ConnectionConfig;
use thaan_core::dialect::Dialect;
use thaan_core::driver::{Driver, DriverInfo, ParamKind, ParamSpec, Session};
use thaan_core::schema::{ColumnInfo, ObjectKind, ObjectRef, SchemaObject, TableInfo};
use thaan_core::table::{Filter, FilterOp, PageRequest, StagedChange};
use thaan_core::{Column, Error, Result, ResultSet, Value};

/// Messages kept per session unless the connection sets `buffer`.
pub const DEFAULT_BUFFER: usize = 1000;
/// Topics listed in the tree.
pub const TOPIC_LIMIT: usize = 1000;
/// Payload bytes kept per message; longer payloads are cut (the `bytes` column keeps the
/// real size).
pub const PAYLOAD_CAP: usize = 256 * 1024;
const ACK_TIMEOUT: Duration = Duration::from_secs(10);
/// After SUBACK, how long connect waits for retained messages before returning.
const RETAINED_GRACE: Duration = Duration::from_millis(150);

pub struct MqttDriver;

impl Driver for MqttDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mqtt".into(),
            name: "MQTT".into(),
            params: vec![
                ParamSpec::new("host", "Host", ParamKind::Text).default("localhost"),
                ParamSpec::new("port", "Port", ParamKind::Number).default("1883"),
                ParamSpec::new("user", "User", ParamKind::Text).placeholder("(optional)"),
                ParamSpec::new("password", "Password", ParamKind::Password),
                ParamSpec::new("topic", "Subscribe to", ParamKind::Text)
                    .default("#")
                    .placeholder("# or sensors/+/temp"),
                ParamSpec::new("client_id", "Client ID", ParamKind::Text)
                    .placeholder("(generated)"),
                ParamSpec::new("buffer", "Messages kept", ParamKind::Number)
                    .default(&DEFAULT_BUFFER.to_string()),
            ],
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::MQTT
    }

    fn connect(&self, c: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let host = c.param("host").unwrap_or("localhost");
        let port: u16 = c
            .param("port")
            .unwrap_or("1883")
            .parse()
            .map_err(|_| Error::InvalidParams("port must be a number".into()))?;
        let buffer: usize = c
            .param("buffer")
            .map(|b| {
                b.parse()
                    .map_err(|_| Error::InvalidParams("messages kept must be a number".into()))
            })
            .transpose()?
            .unwrap_or(DEFAULT_BUFFER)
            .clamp(1, 100_000);
        let filter = c.param("topic").unwrap_or("#").to_string();
        let client_id = c.param("client_id").map(str::to_string).unwrap_or_else(|| {
            let n = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            format!("thaan-{:x}", (n as u64) ^ u64::from(std::process::id()))
        });
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(|e| Error::Database(format!("{host}: {e}")))?
            .next()
            .ok_or_else(|| Error::Database(format!("{host}: no address")))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(io)?;
        stream.set_nodelay(true).ok();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(io)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(io)?;

        let keep_alive = 60u16;
        stream
            .write_all(&packet::connect(&packet::Connect {
                client_id: &client_id,
                user: c.param("user"),
                password: c.param("password"),
                keep_alive,
            }))
            .map_err(io)?;
        let mut reader = BufReader::new(stream.try_clone().map_err(io)?);
        match packet::read(&mut reader).map_err(io)? {
            Incoming::ConnAck { code: 0, .. } => {}
            Incoming::ConnAck { code, .. } => {
                return Err(Error::Database(format!(
                    "MQTT: {}",
                    packet::connack_error(code)
                )))
            }
            other => {
                return Err(Error::Database(format!(
                    "MQTT: expected CONNACK, got {other:?}"
                )))
            }
        }
        // From here the reader thread blocks on the socket; it ends when we shut it down.
        stream.set_read_timeout(None).map_err(io)?;

        let shared = Arc::new(Shared {
            writer: Mutex::new(stream),
            state: Mutex::new(State {
                buffer_cap: buffer,
                ..Default::default()
            }),
            cond: Condvar::new(),
            closed: AtomicBool::new(false),
            next_id: AtomicU16::new(1),
        });
        {
            let sh = shared.clone();
            std::thread::Builder::new()
                .name("mqtt-reader".into())
                .spawn(move || reader_loop(sh, reader))
                .map_err(io)?;
        }
        {
            let sh = shared.clone();
            let every = Duration::from_secs(u64::from(keep_alive) / 2);
            std::thread::Builder::new()
                .name("mqtt-ping".into())
                .spawn(move || ping_loop(sh, every))
                .map_err(io)?;
        }
        let mut s = MqttSession {
            shared,
            read_only: c.read_only,
            filter: filter.clone(),
        };
        s.subscribe(&filter, 1)?;
        std::thread::sleep(RETAINED_GRACE);
        Ok(Box::new(s))
    }
}

fn io(e: std::io::Error) -> Error {
    Error::Database(e.to_string())
}

#[derive(Debug, Clone)]
struct Message {
    /// Set by `Shared::record`, increasing per session.
    id: u64,
    /// Published by this session rather than received.
    sent: bool,
    at: SystemTime,
    topic: String,
    payload: Vec<u8>,
    size: usize,
    qos: u8,
    retain: bool,
}

#[derive(Debug, Clone, Default)]
struct TopicStat {
    count: u64,
    last: Option<SystemTime>,
}

#[derive(Default)]
struct State {
    messages: VecDeque<Message>,
    buffer_cap: usize,
    topics: BTreeMap<String, TopicStat>,
    /// Replies to our SUBSCRIBE / UNSUBSCRIBE / PUBLISH (QoS 1), by packet id.
    acks: HashMap<u16, Vec<u8>>,
    /// Set when the reader thread stops (connection lost).
    error: Option<String>,
    /// Id of the last recorded message.
    last_id: u64,
    /// Filters this session is subscribed to, with the QoS the broker granted.
    subs: BTreeMap<String, u8>,
}

struct Shared {
    writer: Mutex<TcpStream>,
    state: Mutex<State>,
    cond: Condvar,
    closed: AtomicBool,
    next_id: AtomicU16,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // A panic in another thread while holding the lock leaves plain data; keep going.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(bytes)?;
        w.flush()
    }

    fn packet_id(&self) -> u16 {
        loop {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                return id;
            }
        }
    }

    fn record(&self, mut m: Message) {
        let mut st = self.state();
        st.last_id += 1;
        m.id = st.last_id;
        let stat = st.topics.entry(m.topic.clone()).or_default();
        stat.count += 1;
        stat.last = Some(m.at);
        if st.topics.len() > TOPIC_LIMIT * 10 {
            // Pathological brokers with endless unique topics: forget the quietest ones.
            let mut v: Vec<_> = st.topics.iter().map(|(k, s)| (s.last, k.clone())).collect();
            v.sort();
            for (_, k) in v.into_iter().take(TOPIC_LIMIT) {
                st.topics.remove(&k);
            }
        }
        while st.messages.len() >= st.buffer_cap {
            st.messages.pop_front();
        }
        st.messages.push_back(m);
    }
}

fn reader_loop(sh: Arc<Shared>, mut r: BufReader<TcpStream>) {
    let err = loop {
        match packet::read(&mut r) {
            Ok(Incoming::Publish {
                topic,
                mut payload,
                qos,
                retain,
                packet_id,
            }) => {
                match (qos, packet_id) {
                    (1, Some(id)) => {
                        sh.send(&packet::puback(id)).ok();
                    }
                    (2, Some(id)) => {
                        sh.send(&packet::pubrec(id)).ok();
                    }
                    _ => {}
                }
                let size = payload.len();
                payload.truncate(PAYLOAD_CAP);
                sh.record(Message {
                    id: 0,
                    sent: false,
                    at: SystemTime::now(),
                    topic,
                    payload,
                    size,
                    qos,
                    retain,
                });
            }
            Ok(Incoming::PubRel(id)) => {
                sh.send(&packet::pubcomp(id)).ok();
            }
            Ok(Incoming::SubAck { packet_id, codes }) => {
                sh.state().acks.insert(packet_id, codes);
                sh.cond.notify_all();
            }
            Ok(Incoming::PubAck(id) | Incoming::UnsubAck(id) | Incoming::PubComp(id)) => {
                sh.state().acks.insert(id, Vec::new());
                sh.cond.notify_all();
            }
            Ok(Incoming::PubRec(id)) => {
                sh.send(&packet::pubrel(id)).ok();
            }
            Ok(_) => {}
            Err(e) => break e.to_string(),
        }
    };
    if !sh.closed.load(Ordering::Relaxed) {
        sh.state().error = Some(format!("connection to the broker was lost: {err}"));
    }
    sh.closed.store(true, Ordering::Relaxed);
    sh.cond.notify_all();
}

fn ping_loop(sh: Arc<Shared>, every: Duration) {
    let mut last = Instant::now();
    while !sh.closed.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        if last.elapsed() >= every {
            if sh.send(&packet::pingreq()).is_err() {
                break;
            }
            last = Instant::now();
        }
    }
}

pub struct MqttSession {
    shared: Arc<Shared>,
    read_only: bool,
    /// The filter subscribed at connect; shown as the first object.
    filter: String,
}

impl Drop for MqttSession {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.send(&packet::disconnect()).ok();
        let w = self.shared.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.shutdown(Shutdown::Both).ok();
    }
}

impl MqttSession {
    fn alive(&self) -> Result<()> {
        if let Some(e) = &self.shared.state().error {
            return Err(Error::Database(e.clone()));
        }
        Ok(())
    }

    /// Sends a packet that the broker answers by packet id, and waits for the answer.
    fn request(&self, id: u16, bytes: &[u8]) -> Result<Vec<u8>> {
        self.alive()?;
        self.shared.send(bytes).map_err(io)?;
        let deadline = Instant::now() + ACK_TIMEOUT;
        let mut st = self.shared.state();
        loop {
            if let Some(codes) = st.acks.remove(&id) {
                return Ok(codes);
            }
            if let Some(e) = &st.error {
                return Err(Error::Database(e.clone()));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Database("the broker did not answer in time".into()));
            }
            st = self
                .shared
                .cond
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn subscribe(&mut self, filter: &str, qos: u8) -> Result<u8> {
        let id = self.shared.packet_id();
        let codes = self.request(id, &packet::subscribe(id, &[(filter, qos.min(1))]))?;
        match codes.first() {
            Some(0x80) | None => Err(Error::Database(format!(
                "the broker refused the subscription to `{filter}`"
            ))),
            Some(&granted) => {
                self.shared.state().subs.insert(filter.to_string(), granted);
                Ok(granted)
            }
        }
    }

    fn publish(&mut self, topic: &str, payload: &[u8], qos: u8, retain: bool) -> Result<()> {
        if self.read_only {
            return Err(Error::ReadOnly("PUBLISH".into()));
        }
        if topic.is_empty() || topic.contains(['+', '#']) {
            return Err(Error::InvalidParams(format!(
                "cannot publish to `{topic}`: topics take no wildcards"
            )));
        }
        let qos = qos.min(1);
        let id = if qos > 0 { self.shared.packet_id() } else { 0 };
        let bytes = packet::publish(topic, payload, qos, retain, id);
        if qos == 0 {
            self.alive()?;
            self.shared.send(&bytes).map_err(io)?;
        } else {
            self.request(id, &bytes)?;
        }
        self.shared.record(Message {
            id: 0,
            sent: true,
            at: SystemTime::now(),
            topic: topic.to_string(),
            payload: payload[..payload.len().min(PAYLOAD_CAP)].to_vec(),
            size: payload.len(),
            qos,
            retain,
        });
        Ok(())
    }

    /// Received messages matching `filter`, newest first.
    fn matching(&self, filter: &str) -> Vec<Message> {
        self.shared
            .state()
            .messages
            .iter()
            .rev()
            .filter(|m| !m.sent && topic_matches(filter, &m.topic))
            .cloned()
            .collect()
    }
}

fn payload_value(p: &[u8]) -> Value {
    match std::str::from_utf8(p) {
        Ok(s) => Value::Text(s.to_string()),
        Err(_) => Value::Bytes(p.to_vec()),
    }
}

/// UTC time as `2026-11-26 14:03:07.512` (civil-from-days, Howard Hinnant's algorithm).
fn timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60,
        d.subsec_millis()
    )
}

const MESSAGE_COLUMNS: [&str; 6] = ["received_at", "topic", "payload", "qos", "retain", "bytes"];

fn message_row(m: &Message) -> Vec<Value> {
    vec![
        Value::Text(timestamp(m.at)),
        Value::Text(m.topic.clone()),
        payload_value(&m.payload),
        Value::Int(i64::from(m.qos)),
        Value::Bool(m.retain),
        Value::Int(m.size as i64),
    ]
}

fn message_result(rows: Vec<Vec<Value>>) -> ResultSet {
    ResultSet {
        columns: MESSAGE_COLUMNS.iter().map(|c| Column::new(*c)).collect(),
        rows,
        ..Default::default()
    }
}

/// SQL-style `like` on text: `%` any run, `_` one character.
fn like(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('%', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some(('_', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

/// Whether a message row passes the table view's filters (evaluated in memory: the
/// buffer is small and bounded).
fn row_passes(row: &[Value], filters: &[Filter]) -> Result<bool> {
    for f in filters {
        let i = MESSAGE_COLUMNS
            .iter()
            .position(|c| *c == f.column)
            .ok_or_else(|| Error::InvalidParams(format!("no column `{}`", f.column)))?;
        let v = &row[i];
        let text = v.to_string();
        let want = f.values.first().map(|x| x.to_string()).unwrap_or_default();
        let num = |x: &Value| match x {
            Value::Int(n) => Some(*n as f64),
            Value::Float(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Text(s) => s.trim().parse().ok(),
            _ => None,
        };
        let cmp = || match (num(v), f.values.first().and_then(num)) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(text.as_str().cmp(want.as_str())),
        };
        let ok = match f.op {
            FilterOp::Eq => {
                text == want || (num(v).is_some() && cmp() == Some(std::cmp::Ordering::Equal))
            }
            FilterOp::Ne => text != want,
            FilterOp::Lt => cmp() == Some(std::cmp::Ordering::Less),
            FilterOp::Le => cmp() != Some(std::cmp::Ordering::Greater),
            FilterOp::Gt => cmp() == Some(std::cmp::Ordering::Greater),
            FilterOp::Ge => cmp() != Some(std::cmp::Ordering::Less),
            FilterOp::Like => like(&want, &text),
            FilterOp::NotLike => !like(&want, &text),
            FilterOp::IsNull => v.is_null(),
            FilterOp::IsNotNull => !v.is_null(),
            FilterOp::In => f.values.iter().any(|x| x.to_string() == text),
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

fn text_arg(a: &[u8]) -> Result<String> {
    String::from_utf8(a.to_vec()).map_err(|_| Error::InvalidParams("topics must be UTF-8".into()))
}

/// `QOS n` / `RETAIN` options after the fixed arguments.
fn options(rest: &[Vec<u8>]) -> Result<(u8, bool)> {
    let mut qos = 0;
    let mut retain = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match String::from_utf8_lossy(a).to_ascii_uppercase().as_str() {
            "RETAIN" => retain = true,
            "QOS" => {
                qos = it
                    .next()
                    .and_then(|q| std::str::from_utf8(q).ok()?.parse().ok())
                    .filter(|q: &u8| *q <= 2)
                    .ok_or_else(|| Error::InvalidParams("QOS takes 0, 1 or 2".into()))?;
            }
            other => return Err(Error::InvalidParams(format!("unknown option `{other}`"))),
        }
    }
    Ok((qos, retain))
}

fn status(text: impl Into<String>) -> ResultSet {
    ResultSet {
        columns: vec![Column::new("reply")],
        rows: vec![vec![Value::Text(text.into())]],
        ..Default::default()
    }
}

const HELP: &str = "PUBLISH <topic> <payload> [QOS 0|1] [RETAIN] · SUBSCRIBE <filter> [QOS 0|1] · UNSUBSCRIBE <filter> · MESSAGES [filter] [limit] · TOPICS · CLEAR";

impl Session for MqttSession {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<ResultSet> {
        let args = split_args(sql).map_err(Error::InvalidParams)?;
        let Some(first) = args.first() else {
            return Err(Error::InvalidParams(HELP.into()));
        };
        let name = String::from_utf8_lossy(first).to_ascii_uppercase();
        match name.as_str() {
            "PUBLISH" | "PUB" => {
                let (Some(topic), Some(payload)) = (args.get(1), args.get(2)) else {
                    return Err(Error::InvalidParams(
                        "PUBLISH <topic> <payload> [QOS 0|1] [RETAIN]".into(),
                    ));
                };
                let topic = text_arg(topic)?;
                let (qos, retain) = options(&args[3..])?;
                self.publish(&topic, payload, qos, retain)?;
                let mut rs = status(format!("published {} bytes to {topic}", payload.len()));
                rs.rows_affected = Some(1);
                Ok(rs)
            }
            "SUBSCRIBE" | "SUB" => {
                let filter = text_arg(
                    args.get(1)
                        .ok_or_else(|| Error::InvalidParams("SUBSCRIBE <filter>".into()))?,
                )?;
                let (qos, _) = options(&args[2..])?;
                let granted = self.subscribe(&filter, qos)?;
                Ok(status(format!("subscribed to {filter} (QoS {granted})")))
            }
            "UNSUBSCRIBE" | "UNSUB" => {
                let filter = text_arg(
                    args.get(1)
                        .ok_or_else(|| Error::InvalidParams("UNSUBSCRIBE <filter>".into()))?,
                )?;
                let id = self.shared.packet_id();
                self.request(id, &packet::unsubscribe(id, &[&filter]))?;
                self.shared.state().subs.remove(&filter);
                Ok(status(format!("unsubscribed from {filter}")))
            }
            "MESSAGES" | "SELECT" => {
                let filter = match args.get(1) {
                    Some(f) => text_arg(f)?,
                    None => "#".into(),
                };
                let limit: usize = match args.get(2) {
                    Some(n) => std::str::from_utf8(n)
                        .ok()
                        .and_then(|n| n.parse().ok())
                        .ok_or_else(|| Error::InvalidParams("limit must be a number".into()))?,
                    None => 100,
                };
                self.alive()?;
                Ok(message_result(
                    self.matching(&filter)
                        .iter()
                        .take(limit)
                        .map(message_row)
                        .collect(),
                ))
            }
            "TOPICS" => {
                let st = self.shared.state();
                let rows = st
                    .topics
                    .iter()
                    .map(|(t, s)| {
                        vec![
                            Value::Text(t.clone()),
                            Value::Int(s.count as i64),
                            s.last.map_or(Value::Null, |l| Value::Text(timestamp(l))),
                        ]
                    })
                    .collect();
                Ok(ResultSet {
                    columns: ["topic", "messages", "last_received_at"]
                        .iter()
                        .map(|c| Column::new(*c))
                        .collect(),
                    rows,
                    ..Default::default()
                })
            }
            "CLEAR" => {
                let mut st = self.shared.state();
                let n = st.messages.len();
                st.messages.clear();
                Ok(status(format!("cleared {n} buffered messages")))
            }
            "HELP" => Ok(status(HELP)),
            other => Err(Error::InvalidParams(format!(
                "unknown MQTT command `{other}`. {HELP}"
            ))),
        }
    }

    fn messages_since(&mut self, since: u64, limit: usize) -> Option<Result<Vec<BrokerMessage>>> {
        Some(self.alive().map(|_| {
            let st = self.shared.state();
            // Ids increase along the buffer, so skip from the back to the first new one.
            let new = st
                .messages
                .iter()
                .rev()
                .take_while(|m| m.id > since)
                .count();
            st.messages
                .iter()
                .skip(st.messages.len() - new)
                .take(limit)
                .map(|m| BrokerMessage {
                    id: m.id,
                    at_ms: m
                        .at
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64),
                    topic: m.topic.clone(),
                    payload: m.payload.clone(),
                    size: m.size,
                    qos: m.qos,
                    retain: m.retain,
                    sent: m.sent,
                })
                .collect()
        }))
    }

    fn subscriptions(&mut self) -> Option<Result<Vec<Subscription>>> {
        Some(Ok(self
            .shared
            .state()
            .subs
            .iter()
            .map(|(f, q)| Subscription {
                filter: f.clone(),
                qos: *q,
            })
            .collect()))
    }

    fn ping(&mut self) -> Result<()> {
        self.alive()?;
        self.shared.send(&packet::pingreq()).map_err(io)
    }

    fn begin(&mut self) -> Result<()> {
        Ok(())
    }
    fn commit(&mut self) -> Result<()> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<()> {
        Ok(())
    }

    fn list_objects(&mut self) -> Result<Vec<SchemaObject>> {
        self.alive()?;
        let obj = |name: String| SchemaObject {
            object: ObjectRef::new(name),
            kind: ObjectKind::Table,
            parent: None,
        };
        let mut out = vec![obj(self.filter.clone())];
        out.extend(
            self.shared
                .state()
                .topics
                .keys()
                .filter(|t| **t != self.filter)
                .take(TOPIC_LIMIT)
                .cloned()
                .map(obj),
        );
        Ok(out)
    }

    fn describe_table(&mut self, t: &ObjectRef) -> Result<TableInfo> {
        let types = ["timestamp", "topic", "payload", "qos", "bool", "integer"];
        Ok(TableInfo {
            object: t.clone(),
            columns: MESSAGE_COLUMNS
                .iter()
                .zip(types)
                .map(|(n, ty)| ColumnInfo {
                    name: n.to_string(),
                    data_type: ty.to_string(),
                    nullable: false,
                    default: None,
                    primary_key: None,
                    comment: None,
                    auto_increment: false,
                })
                .collect(),
            foreign_keys: vec![],
            indexes: vec![],
            triggers: vec![],
        })
    }

    fn ddl(&mut self, o: &ObjectRef) -> Result<String> {
        let st = self.shared.state();
        let seen: Vec<_> = st
            .topics
            .iter()
            .filter(|(t, _)| topic_matches(&o.name, t))
            .collect();
        let count: u64 = seen.iter().map(|(_, s)| s.count).sum();
        let mut out = format!(
            "# {} · {} topic(s) · {count} message(s) received this session\nSUBSCRIBE {}\n",
            o.name,
            seen.len(),
            quote_arg(&o.name)
        );
        if !o.name.contains(['+', '#']) {
            out.push_str(&format!(
                "# PUBLISH {} <payload> [QOS 0|1] [RETAIN]\n",
                quote_arg(&o.name)
            ));
        }
        Ok(out)
    }

    fn fetch_page(&mut self, req: &PageRequest, paged: bool) -> Option<Result<(ResultSet, u64)>> {
        Some((|| {
            self.alive()?;
            if req
                .raw_where
                .as_deref()
                .is_some_and(|r| !r.trim().is_empty())
            {
                return Err(Error::Unsupported(
                    "a raw filter on MQTT messages; use the filter rows".into(),
                ));
            }
            let mut rows = Vec::new();
            for m in self.matching(&req.table.name) {
                let row = message_row(&m);
                if row_passes(&row, &req.filters)? {
                    rows.push(row);
                }
            }
            if let Some(s) = req.sort.first() {
                if let Some(i) = MESSAGE_COLUMNS.iter().position(|c| *c == s.column) {
                    rows.sort_by(|a, b| {
                        let o = a[i].to_string().cmp(&b[i].to_string());
                        if s.descending {
                            o.reverse()
                        } else {
                            o
                        }
                    });
                }
            }
            let total = rows.len() as u64;
            if paged {
                let size = req.page_size.max(1) as usize;
                rows = rows
                    .into_iter()
                    .skip(req.page as usize * size)
                    .take(size)
                    .collect();
            }
            let mut rs = message_result(rows);
            rs.sql = format!("MESSAGES {}", quote_arg(&req.table.name));
            Ok((rs, total))
        })())
    }

    fn preview_changes(&mut self, t: &ObjectRef, changes: &[StagedChange]) -> Option<Vec<String>> {
        Some(
            changes
                .iter()
                .map(|c| match plan(&t.name, c) {
                    Ok(p) => format!(
                        "PUBLISH {} {}{}{}",
                        quote_arg(&p.topic),
                        quote_arg(&String::from_utf8_lossy(&p.payload)),
                        if p.qos > 0 {
                            format!(" QOS {}", p.qos)
                        } else {
                            String::new()
                        },
                        if p.retain { " RETAIN" } else { "" }
                    ),
                    Err(e) => format!("# {e}"),
                })
                .collect(),
        )
    }

    fn apply_changes(&mut self, t: &ObjectRef, changes: &[StagedChange]) -> Option<Result<u64>> {
        Some((|| {
            let plans = changes
                .iter()
                .map(|c| plan(&t.name, c))
                .collect::<Result<Vec<_>>>()?;
            for p in &plans {
                self.publish(&p.topic, &p.payload, p.qos, p.retain)?;
            }
            Ok(plans.len() as u64)
        })())
    }
}

struct Publish {
    topic: String,
    payload: Vec<u8>,
    qos: u8,
    retain: bool,
}

/// A staged row as a publish: a new row publishes its payload; deleting a retained row
/// clears that topic's retained message.
fn plan(table: &str, c: &StagedChange) -> Result<Publish> {
    let get = |cols: &[(String, Value)], k: &str| {
        cols.iter().find(|(c, _)| c == k).map(|(_, v)| v.clone())
    };
    match c {
        StagedChange::Insert { values } => {
            let topic = match get(values, "topic").filter(|v| !v.is_null()) {
                Some(v) => v.to_string(),
                None => table.to_string(),
            };
            let payload = match get(values, "payload") {
                Some(Value::Bytes(b)) => b,
                Some(Value::Null) | None => Vec::new(),
                Some(v) => v.to_string().into_bytes(),
            };
            let qos = match get(values, "qos") {
                Some(Value::Int(q)) if (0..=1).contains(&q) => q as u8,
                Some(Value::Text(q)) => q.trim().parse().unwrap_or(0).min(1),
                _ => 0,
            };
            let retain = matches!(get(values, "retain"), Some(Value::Bool(true)))
                || matches!(get(values, "retain"), Some(Value::Text(t)) if t == "true" || t == "1");
            Ok(Publish {
                topic,
                payload,
                qos,
                retain,
            })
        }
        StagedChange::Delete { key } => {
            let retained = matches!(get(key, "retain"), Some(Value::Bool(true)));
            let topic = get(key, "topic").map(|v| v.to_string()).unwrap_or_default();
            if !retained || topic.is_empty() {
                return Err(Error::Unsupported(
                    "only retained messages can be deleted (a broker does not keep other messages)"
                        .into(),
                ));
            }
            Ok(Publish {
                topic,
                payload: Vec::new(),
                qos: 0,
                retain: true,
            })
        }
        StagedChange::Update { .. } => Err(Error::Unsupported(
            "messages cannot be edited; add a row to publish a new one".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        let t = UNIX_EPOCH + Duration::from_millis(1_795_000_000_123);
        assert_eq!(timestamp(t), "2026-11-18 11:06:40.123");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01 00:00:00.000");
    }

    #[test]
    fn like_patterns() {
        assert!(like("temp%", "temperature"));
        assert!(like("%/t_mp", "a/b/temp"));
        assert!(!like("temp", "temps"));
    }

    #[test]
    fn filters_on_rows() {
        let row = message_row(&Message {
            id: 1,
            sent: false,
            at: UNIX_EPOCH,
            topic: "a/b".into(),
            payload: b"21.5".to_vec(),
            size: 4,
            qos: 1,
            retain: false,
        });
        let f = |column: &str, op, v: Value| Filter {
            column: column.into(),
            op,
            values: vec![v],
        };
        assert!(row_passes(&row, &[f("payload", FilterOp::Gt, Value::Int(20))]).unwrap());
        assert!(!row_passes(&row, &[f("qos", FilterOp::Eq, Value::Int(0))]).unwrap());
        assert!(row_passes(&row, &[f("topic", FilterOp::Like, Value::from("a/%"))]).unwrap());
        assert!(row_passes(&row, &[f("nope", FilterOp::Eq, Value::Null)]).is_err());
    }

    #[test]
    fn staged_rows_become_publishes() {
        let p = plan(
            "sensors/t",
            &StagedChange::Insert {
                values: vec![
                    ("payload".into(), Value::from("22")),
                    ("retain".into(), Value::Bool(true)),
                ],
            },
        )
        .unwrap();
        assert_eq!(
            (p.topic.as_str(), p.payload.as_slice(), p.retain),
            ("sensors/t", &b"22"[..], true)
        );
        let clear = plan(
            "#",
            &StagedChange::Delete {
                key: vec![
                    ("topic".into(), Value::from("x/y")),
                    ("retain".into(), Value::Bool(true)),
                ],
            },
        )
        .unwrap();
        assert!(clear.payload.is_empty() && clear.retain && clear.topic == "x/y");
        assert!(plan(
            "#",
            &StagedChange::Delete {
                key: vec![("topic".into(), Value::from("x"))]
            }
        )
        .is_err());
    }

    /// Needs a broker: THAAN_TEST_MQTT_URL=mqtt://127.0.0.1:51883
    #[test]
    #[ignore]
    fn live_publish_and_browse() {
        let url = std::env::var("THAAN_TEST_MQTT_URL").expect("THAAN_TEST_MQTT_URL");
        let mut cfg = ConnectionConfig::from_url(&url).unwrap();
        cfg.params.insert("topic".into(), "thaan-test/#".into());
        let mut s = MqttDriver.connect(&cfg).unwrap();
        s.execute("PUBLISH thaan-test/retained \"kept\" RETAIN", &[])
            .unwrap();
        s.execute("PUBLISH thaan-test/a/temp 21.5 QOS 1", &[])
            .unwrap();
        s.execute("PUBLISH thaan-test/a/temp 22.0 QOS 1", &[])
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let names: Vec<_> = s
            .list_objects()
            .unwrap()
            .into_iter()
            .map(|o| o.object.name)
            .collect();
        assert_eq!(
            names,
            ["thaan-test/#", "thaan-test/a/temp", "thaan-test/retained"]
        );
        let req = PageRequest {
            table: ObjectRef::new("thaan-test/+/temp"),
            filters: vec![],
            raw_where: None,
            projection: None,
            raw_sort: None,
            sort: vec![],
            page: 0,
            page_size: 50,
        };
        let (rs, total) = s.fetch_page(&req, true).unwrap().unwrap();
        assert_eq!(total, 2);
        assert_eq!(rs.rows[0][2], Value::from("22.0"), "newest first");

        // A second session sees the retained message straight away.
        let mut other = MqttDriver.connect(&cfg).unwrap();
        let rs = other.execute("MESSAGES thaan-test/retained", &[]).unwrap();
        assert_eq!(rs.rows.len(), 1);
        assert_eq!(rs.rows[0][4], Value::Bool(true));
        // Deleting it clears the retained message for the next subscriber.
        other
            .apply_changes(
                &ObjectRef::new("thaan-test/#"),
                &[StagedChange::Delete {
                    key: vec![
                        ("topic".into(), Value::from("thaan-test/retained")),
                        ("retain".into(), Value::Bool(true)),
                    ],
                }],
            )
            .unwrap()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let mut third = MqttDriver.connect(&cfg).unwrap();
        assert!(third
            .execute("MESSAGES thaan-test/retained", &[])
            .unwrap()
            .rows
            .is_empty());

        // The console's view: subscriptions, then messages after a known id.
        let subs = s.subscriptions().unwrap().unwrap();
        assert_eq!(subs[0].filter, "thaan-test/#");
        let all = s.messages_since(0, 100).unwrap().unwrap();
        assert!(all.iter().any(|m| m.sent && m.topic == "thaan-test/a/temp"));
        assert!(all
            .iter()
            .any(|m| !m.sent && m.topic == "thaan-test/a/temp"));
        let last = all.last().unwrap().id;
        assert!(s.messages_since(last, 100).unwrap().unwrap().is_empty());
        s.execute("UNSUBSCRIBE thaan-test/#", &[]).unwrap();
        assert!(s.subscriptions().unwrap().unwrap().is_empty());

        let mut ro = cfg.clone();
        ro.read_only = true;
        let mut r = MqttDriver.connect(&ro).unwrap();
        assert!(matches!(
            r.execute("PUBLISH thaan-test/x 1", &[]),
            Err(Error::ReadOnly(_))
        ));
    }
}
