//! Typed views for stores that are not tables: a key-value keyspace (Redis) browsed with
//! a cursor, and a message broker (MQTT) read as a stream of buffered messages.
//! Drivers opt in through the `Session` hooks `scan_keys`, `messages_since` and
//! `subscriptions`; the GUI draws a key browser and a broker console from these.

use serde::{Deserialize, Serialize};

/// One step of a keyspace scan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyScan {
    /// Glob such as `user:*`; `None` or empty matches every key.
    #[serde(default)]
    pub pattern: Option<String>,
    /// Only keys of this type (`string`, `hash`, `list`, `set`, `zset`, `stream`).
    #[serde(default)]
    pub key_type: Option<String>,
    /// `"0"` (or empty) starts a scan; pass back the cursor a page returned to go on.
    #[serde(default)]
    pub cursor: String,
    /// How many keys to aim for in this step (a hint; a step may return fewer or more).
    #[serde(default = "default_count")]
    pub count: u32,
}

fn default_count() -> u32 {
    200
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEntry {
    pub name: String,
    pub key_type: String,
    /// Seconds left; `None` when the key never expires.
    #[serde(default)]
    pub ttl: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyPage {
    pub keys: Vec<KeyEntry>,
    /// Cursor for the next step; `"0"` when the scan is complete.
    pub cursor: String,
    /// Keys in the whole database, when cheap to know.
    #[serde(default)]
    pub total: Option<u64>,
}

impl KeyPage {
    pub fn done(&self) -> bool {
        self.cursor.is_empty() || self.cursor == "0"
    }
}

/// A message a broker session received, or one it published (`sent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerMessage {
    /// Increasing per session; ask for messages after the last id you have.
    pub id: u64,
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub topic: String,
    /// The payload as received (cut at the driver's cap; `size` keeps the real length).
    pub payload: Vec<u8>,
    pub size: usize,
    pub qos: u8,
    pub retain: bool,
    pub sent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub filter: String,
    pub qos: u8,
}
