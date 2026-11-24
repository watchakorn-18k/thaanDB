use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A saved connection. Parameters are driver-specific and described by
/// [`crate::driver::DriverInfo::params`]. Passwords are never persisted (see `store`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    /// Stable id; generated when empty on save.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Driver id, e.g. `sqlite`, `postgres`.
    pub driver: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// Colour label shown in the sidebar and title bar, e.g. `#14b8a6`.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    /// The engine refuses write statements on read-only connections.
    #[serde(default)]
    pub read_only: bool,
}

impl ConnectionConfig {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .get(key)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Parses a connection URL such as `postgres://user:pw@host:5432/db?sslmode=require`,
    /// `mysql://root@localhost/app`, `sqlite:./app.db` or `sqlite::memory:`.
    pub fn from_url(url: &str) -> Result<ConnectionConfig> {
        let url = url.trim();
        let (scheme, rest) = url
            .split_once(':')
            .ok_or_else(|| Error::InvalidParams(format!("not a URL: {url}")))?;
        let driver = match scheme.to_ascii_lowercase().as_str() {
            "sqlite" | "sqlite3" | "file" => "sqlite",
            "postgres" | "postgresql" => "postgres",
            "mysql" | "mariadb" => "mysql",
            "sqlserver" | "mssql" => "mssql",
            "cockroachdb" | "cockroach" => "postgres",
            "mongodb" | "mongodb+srv" => "mongodb",
            other => return Err(Error::UnknownDriver(other.to_string())),
        }
        .to_string();

        let mut params = BTreeMap::new();
        if driver == "mongodb" {
            // MongoDB URIs can list several hosts and many options; keep the URI whole but
            // move the credentials out so the password is never persisted inside it.
            let body = rest.strip_prefix("//").unwrap_or(rest);
            // The last `@` before the path separates credentials (tolerates a raw `@`
            // in the password).
            let authority_end = body.find('/').unwrap_or(body.len());
            let (userinfo, hosts) = match body[..authority_end].rfind('@') {
                Some(at) => (Some(&body[..at]), &body[at + 1..]),
                None => (None, body),
            };
            if let Some(u) = userinfo {
                let (user, pass) = u.split_once(':').unwrap_or((u, ""));
                params.insert("user".into(), percent_decode(user));
                if !pass.is_empty() {
                    params.insert("password".into(), percent_decode(pass));
                }
            }
            let path = hosts.split_once('?').map(|x| x.0).unwrap_or(hosts);
            let host_part = path.split('/').next().unwrap_or("");
            if let Some(db) = path.split_once('/').map(|x| x.1).filter(|d| !d.is_empty()) {
                params.insert("database".into(), percent_decode(db));
            }
            params.insert("url".into(), format!("{scheme}://{hosts}"));
            let name = match params.get("database") {
                Some(db) => format!("{db}@{host_part}"),
                None => host_part.to_string(),
            };
            return Ok(ConnectionConfig { name, driver, params, ..Default::default() });
        }
        if driver == "sqlite" {
            let path = rest.strip_prefix("//").unwrap_or(rest);
            let path = if path == ":memory:" || path.is_empty() {
                ":memory:"
            } else {
                path
            };
            params.insert("path".into(), percent_decode(path));
            let name = if path == ":memory:" {
                "in-memory".to_string()
            } else {
                path.rsplit('/').next().unwrap_or(path).to_string()
            };
            return Ok(ConnectionConfig {
                name,
                driver,
                params,
                ..Default::default()
            });
        }

        let rest = rest
            .strip_prefix("//")
            .ok_or_else(|| Error::InvalidParams(format!("expected `{scheme}://…`")))?;
        let (rest, query) = match rest.split_once('?') {
            Some((a, q)) => (a, Some(q)),
            None => (rest, None),
        };
        let (authority, database) = match rest.split_once('/') {
            Some((a, d)) => (a, d),
            None => (rest, ""),
        };
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, authority),
        };
        if let Some(u) = userinfo {
            let (user, pass) = match u.split_once(':') {
                Some((a, b)) => (a, Some(b)),
                None => (u, None),
            };
            params.insert("user".into(), percent_decode(user));
            if let Some(p) = pass {
                params.insert("password".into(), percent_decode(p));
            }
        }
        let (host, port) = split_host_port(hostport);
        params.insert("host".into(), host.to_string());
        if let Some(p) = port {
            params.insert("port".into(), p.to_string());
        }
        if !database.is_empty() {
            params.insert("database".into(), percent_decode(database));
        }
        for pair in query.unwrap_or("").split('&').filter(|s| !s.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            params.insert(percent_decode(k), percent_decode(v));
        }
        let name = match params.get("database") {
            Some(db) => format!("{db}@{host}"),
            None => host.to_string(),
        };
        Ok(ConnectionConfig {
            name,
            driver,
            params,
            ..Default::default()
        })
    }
}

fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some(rest) = s.strip_prefix('[') {
        // IPv6 literal: [::1]:5432
        if let Some((h, after)) = rest.split_once(']') {
            return (h, after.strip_prefix(':'));
        }
    }
    match s.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, Some(p)),
        _ => (s, None),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_postgres_url() {
        let c =
            ConnectionConfig::from_url("postgres://me:p%40ss@db.local:6543/shop?sslmode=require")
                .unwrap();
        assert_eq!(c.driver, "postgres");
        assert_eq!(c.param("user"), Some("me"));
        assert_eq!(c.param("password"), Some("p@ss"));
        assert_eq!(c.param("host"), Some("db.local"));
        assert_eq!(c.param("port"), Some("6543"));
        assert_eq!(c.param("database"), Some("shop"));
        assert_eq!(c.param("sslmode"), Some("require"));
        assert_eq!(c.name, "shop@db.local");
    }

    #[test]
    fn parses_sqlite_urls() {
        let c = ConnectionConfig::from_url("sqlite::memory:").unwrap();
        assert_eq!(c.param("path"), Some(":memory:"));
        let c = ConnectionConfig::from_url("sqlite:///tmp/a.db").unwrap();
        assert_eq!(c.param("path"), Some("/tmp/a.db"));
        assert_eq!(c.name, "a.db");
    }

    #[test]
    fn parses_mongodb_urls() {
        let c = ConnectionConfig::from_url(
            "mongodb://app:p%40ss@h1:27017,h2:27018/shop?replicaSet=rs0&authSource=admin",
        )
        .unwrap();
        assert_eq!(c.driver, "mongodb");
        assert_eq!(c.param("user"), Some("app"));
        assert_eq!(c.param("password"), Some("p@ss"));
        assert_eq!(c.param("database"), Some("shop"));
        assert_eq!(
            c.param("url"),
            Some("mongodb://h1:27017,h2:27018/shop?replicaSet=rs0&authSource=admin")
        );
        let c = ConnectionConfig::from_url("mongodb+srv://cluster0.example.net").unwrap();
        assert_eq!(c.param("url"), Some("mongodb+srv://cluster0.example.net"));
        assert_eq!(c.param("database"), None);
    }

    #[test]
    fn parses_ipv6_host() {
        let c = ConnectionConfig::from_url("mysql://root@[::1]:3306/app").unwrap();
        assert_eq!(c.param("host"), Some("::1"));
        assert_eq!(c.param("port"), Some("3306"));
    }
}
