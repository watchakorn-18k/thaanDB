//! `thaan mcp`: the MCP stdio transport. One JSON-RPC message per line on stdin, one per
//! line on stdout; logs go to stderr only. The protocol itself is `thaan_core::mcp`.
//!
//! Each request runs on its own thread, so a slow query on one connection does not hold
//! up `ping`, `tools/list` or work on another connection.

use anyhow::Result;
use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thaan_core::engine::Engine;
use thaan_core::mcp::McpServer;
use thaan_core::Error;

/// `store_dir` is where the store lives; the desktop app writes `app-link.json` there.
pub fn serve(engine: Arc<Engine>, store_dir: Option<PathBuf>) -> Result<()> {
    let mut server = McpServer::new(engine);
    if let Some(dir) = store_dir {
        let file = dir.join("app-link.json");
        server = server.with_app_link(Arc::new(move |req: &Json| ask_app(&file, req)));
    }
    let server = Arc::new(server);
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let mut workers = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let (server, out) = (server.clone(), out.clone());
        workers.push(std::thread::spawn(move || {
            if let Some(reply) = server.handle_line(&line) {
                let mut o = out.lock().unwrap_or_else(|p| p.into_inner());
                // A closed stdout means the client is gone; nothing left to tell it.
                let _ = writeln!(o, "{reply}").and_then(|_| o.flush());
            }
        }));
        workers.retain(|w| !w.is_finished());
    }
    // stdin closed: let running requests finish and answer before exiting.
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

/// One request to the running ThaanDB window (see `thaan-app/src/app_link.rs`).
fn ask_app(file: &Path, req: &Json) -> thaan_core::Result<Json> {
    let not_open = || Error::Unsupported("ThaanDB is not open; start the app and try again".into());
    let link: Json = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(not_open)?;
    let port = link["port"].as_u64().ok_or_else(not_open)? as u16;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| not_open())?;
    // The app answers at once (it does not wait for a connect or a query to finish).
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut msg = req.clone();
    msg["token"] = link["token"].clone();
    writeln!(stream, "{msg}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let reply: Json = serde_json::from_str(&line)?;
    if reply["ok"].as_bool() == Some(true) {
        Ok(json!({ "message": reply["message"] }))
    } else {
        Err(Error::Database(
            reply["error"]
                .as_str()
                .unwrap_or("ThaanDB refused")
                .to_string(),
        ))
    }
}
