//! Lets `thaan mcp` show things in this window: open a table, or run a query in a new
//! tab (the `open_table` and `execute_and_show` MCP tools). A child module of `app`.
//!
//! When the window opens, a background thread listens on 127.0.0.1 on a free port and
//! writes `app-link.json` (port, a random token, pid) next to the store, readable only
//! by this user. A client sends one JSON line `{token, action, connection, …}` and gets
//! one line back. Requests reach the UI thread through a channel; nothing here blocks
//! the window. The file is removed when the app quits; a stale one only makes the
//! client say the app is not open.

use super::*;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;
use thaan_core::store::Store;

/// Requests kept while a connect runs.
const LINK_QUEUE: usize = 20;

/// What a client asked the window to do.
pub(super) enum LinkAction {
    OpenTable(ObjectRef),
    /// Run this text in a new query tab of the connection.
    Show(String),
}

struct LinkRequest {
    connection: String,
    action: LinkAction,
    reply: std::sync::mpsc::Sender<Result<String, String>>,
}

pub(super) fn link_file() -> Option<PathBuf> {
    Store::default_path().and_then(|p| p.parent().map(|d| d.join("app-link.json")))
}

/// 128 random bits as hex, from the std hasher's per-process random keys.
fn token() -> String {
    let word = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", word(), word())
}

fn write_link_file(path: &PathBuf, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(body.as_bytes())
}

/// One client connection: read a line, hand it to the UI, write the answer.
fn serve_client(
    stream: TcpStream,
    token: &str,
    tx: &async_channel::Sender<LinkRequest>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let answer = match parse_request(&line, token) {
        Err(e) => Err(e),
        Ok((connection, action)) => {
            let (reply, wait) = std::sync::mpsc::channel();
            let req = LinkRequest {
                connection,
                action,
                reply,
            };
            if tx.send_blocking(req).is_err() {
                Err("ThaanDB is closing".to_string())
            } else {
                wait.recv_timeout(Duration::from_secs(10))
                    .unwrap_or_else(|_| Err("ThaanDB did not answer".into()))
            }
        }
    };
    let out = match answer {
        Ok(message) => serde_json::json!({ "ok": true, "message": message }),
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    };
    let mut stream = stream;
    writeln!(stream, "{out}")
}

fn parse_request(line: &str, token: &str) -> Result<(String, LinkAction), String> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("bad request: {e}"))?;
    if v["token"].as_str() != Some(token) {
        return Err("bad token".into());
    }
    let connection = v["connection"].as_str().unwrap_or_default().to_string();
    let action = match v["action"].as_str() {
        Some("openTable") => LinkAction::OpenTable(ObjectRef {
            schema: v["schema"].as_str().map(String::from),
            name: v["table"]
                .as_str()
                .ok_or("`table` is required")?
                .to_string(),
        }),
        Some("show") => LinkAction::Show(v["sql"].as_str().ok_or("`sql` is required")?.to_string()),
        other => return Err(format!("unknown action {other:?}")),
    };
    Ok((connection, action))
}

impl ThaanApp {
    /// Starts listening for `thaan mcp` (see the module docs). Runs once, at startup;
    /// the socket and the file are set up off the UI thread.
    pub(super) fn start_app_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = link_file() else { return };
        let (tx, rx) = async_channel::unbounded::<LinkRequest>();
        let file = path.clone();
        std::thread::Builder::new()
            .name("app-link".into())
            .spawn(move || {
                let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
                    return;
                };
                let Ok(addr) = listener.local_addr() else {
                    return;
                };
                let token = token();
                let body = serde_json::json!({
                    "port": addr.port(),
                    "token": token,
                    "pid": std::process::id(),
                })
                .to_string();
                if write_link_file(&file, &body).is_err() {
                    return;
                }
                for stream in listener.incoming().flatten() {
                    let (tx, token) = (tx.clone(), token.clone());
                    std::thread::spawn(move || serve_client(stream, &token, &tx).ok());
                }
            })
            .ok();
        cx.on_app_quit(move |_, _| {
            // Only our own file: another window may have replaced it since.
            let ours = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .is_some_and(|v| v["pid"].as_u64() == Some(u64::from(std::process::id())));
            if ours {
                std::fs::remove_file(&path).ok();
            }
            async {}
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(req) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    let answer = this.handle_link(req.connection, req.action, window, cx);
                    req.reply.send(answer).ok();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn handle_link(
        &mut self,
        connection: String,
        action: LinkAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let Some(c) = self
            .connections
            .iter()
            .find(|c| c.id == connection || c.name.eq_ignore_ascii_case(&connection))
            .cloned()
        else {
            return Err(format!(
                "no saved connection named `{connection}` in ThaanDB (save it with add_connection)"
            ));
        };
        // The user asked an agent to show it: bring the window forward.
        window.activate_window();
        cx.activate(true);
        let what = match &action {
            LinkAction::OpenTable(t) => format!("table {}", t.name),
            LinkAction::Show(_) => "the query in a new tab".to_string(),
        };
        if let Some(sid) = self.live_for(&c.id) {
            self.activate(sid, cx);
            self.run_link_action(action, &c.id, window, cx);
            return Ok(format!("Opened {what} on {} in ThaanDB", c.name));
        }
        // While a connect runs, requests wait their turn instead of failing: an agent
        // often calls open_table and execute_and_show back to back.
        if self.link_after_connect.len() >= LINK_QUEUE {
            return Err("ThaanDB has too many requests waiting for a connection".into());
        }
        self.link_after_connect.push((c.id.clone(), action));
        if self.connecting {
            return Ok(format!(
                "ThaanDB is connecting; {what} on {} opens when that is done",
                c.name
            ));
        }
        let name = c.name.clone();
        self.connect(c, window, cx);
        Ok(format!(
            "Connecting to {name} in ThaanDB; {what} opens when it is connected"
        ))
    }

    /// Runs a link action on the connection in front.
    pub(super) fn run_link_action(
        &mut self,
        action: LinkAction,
        connection_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            LinkAction::OpenTable(t) => self.open_table(t, window, cx),
            LinkAction::Show(sql) => {
                let q = SavedQuery {
                    id: String::new(),
                    name: first_line(&sql).chars().take(24).collect(),
                    folder: None,
                    sql,
                    connection_id: Some(connection_id.to_string()),
                    updated_ms: 0,
                };
                self.new_query_tab(Some(q), window, cx);
                if let Some(Tab::Query(t)) = self.tabs.last_mut() {
                    t.saved_id = None;
                }
                self.run(false, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Named imports: `super::*` would bring in GPUI's own `test` attribute.
    use super::{parse_request, token};

    #[test]
    fn requests_need_the_token() {
        let ok = r#"{"token":"t","action":"openTable","connection":"shop","table":"users"}"#;
        assert!(parse_request(ok, "t").is_ok());
        assert_eq!(
            parse_request(ok, "other").err().as_deref(),
            Some("bad token")
        );
        let show = r#"{"token":"t","action":"show","connection":"shop"}"#;
        assert!(parse_request(show, "t").is_err());
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(token(), token());
        assert_eq!(token().len(), 32);
    }
}
