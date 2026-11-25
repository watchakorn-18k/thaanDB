//! `thaan mcp`: the MCP stdio transport. One JSON-RPC message per line on stdin, one per
//! line on stdout; logs go to stderr only. The protocol itself is `thaan_core::mcp`.
//!
//! Each request runs on its own thread, so a slow query on one connection does not hold
//! up `ping`, `tools/list` or work on another connection.

use anyhow::Result;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use thaan_core::engine::Engine;
use thaan_core::mcp::McpServer;

pub fn serve(engine: Arc<Engine>) -> Result<()> {
    let server = Arc::new(McpServer::new(engine));
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
