//! Persistent app data: saved connections (without secrets), saved queries, history.

use crate::connection::ConnectionConfig;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Parameter keys never written to disk.
pub const SECRET_KEYS: &[&str] = &["password", "sshPassword", "sshPassphrase"];
pub const HISTORY_LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub folder: Option<String>,
    pub sql: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub connection_id: String,
    pub sql: String,
    pub at_ms: u64,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub rows: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreData {
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
    #[serde(default)]
    pub saved_queries: Vec<SavedQuery>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

/// JSON file store. `path == None` keeps everything in memory (tests, CLI one-shots).
#[derive(Debug, Default)]
pub struct Store {
    path: Option<PathBuf>,
    pub data: StoreData,
}

impl Store {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreData::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path: Some(path),
            data,
        })
    }

    /// Default location: `$THAAN_HOME/store.json`, else the platform config dir.
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(h) = std::env::var("THAAN_HOME") {
            return Some(PathBuf::from(h).join("store.json"));
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        let home = PathBuf::from(home);
        let base = if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or(home)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or(home.join(".config"))
        };
        Some(base.join("thaandb").join("store.json"))
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut data = self.data.clone();
        for c in &mut data.connections {
            for k in SECRET_KEYS {
                c.params.remove(*k);
            }
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&data)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn push_history(&mut self, mut e: HistoryEntry) {
        if e.id.is_empty() {
            e.id = new_id();
        }
        self.data.history.push(e);
        let n = self.data.history.len();
        if n > HISTORY_LIMIT {
            self.data.history.drain(..n - HISTORY_LIMIT);
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Short unique id (time + counter), no external crate.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:04x}", now_ms(), n & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_strips_secrets_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("thaan-store-{}", new_id()));
        let path = dir.join("store.json");
        let mut s = Store::open(&path).unwrap();
        let mut c = ConnectionConfig::from_url("postgres://u:secret@h/db").unwrap();
        c.id = "c1".into();
        s.data.connections.push(c);
        s.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret"));
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.data.connections[0].param("user"), Some("u"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn history_is_capped() {
        let mut s = Store::in_memory();
        for i in 0..HISTORY_LIMIT + 5 {
            s.push_history(HistoryEntry {
                id: String::new(),
                connection_id: "c".into(),
                sql: format!("select {i}"),
                at_ms: 0,
                elapsed_ms: 0,
                rows: None,
                error: None,
            });
        }
        assert_eq!(s.data.history.len(), HISTORY_LIMIT);
        assert_eq!(s.data.history[0].sql, "select 5");
    }
}
