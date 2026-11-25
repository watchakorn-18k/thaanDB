//! Where connection secrets (passwords, SSH passphrases) live. The store file never
//! holds them (`store::SECRET_KEYS`); a client hands the engine a `SecretStore` backed
//! by the OS keychain, and the engine fills secrets back in when a saved connection is
//! opened. Without one (tests, the wasm build) secrets live only in memory for the run.

use crate::Result;

/// A place to keep one secret per (connection id, parameter key).
///
/// Calls can block (the OS may ask the user to allow access), so callers run them off
/// the UI thread, like any other engine I/O.
pub trait SecretStore: Send + Sync {
    fn get(&self, connection_id: &str, key: &str) -> Option<String>;
    fn set(&self, connection_id: &str, key: &str, value: &str) -> Result<()>;
    fn delete(&self, connection_id: &str, key: &str);
}

/// Keeps secrets in memory only; used by tests.
#[derive(Default)]
pub struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<(String, String), String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, connection_id: &str, key: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&(connection_id.to_string(), key.to_string()))
            .cloned()
    }

    fn set(&self, connection_id: &str, key: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((connection_id.into(), key.into()), value.into());
        Ok(())
    }

    fn delete(&self, connection_id: &str, key: &str) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&(connection_id.to_string(), key.to_string()));
    }
}
