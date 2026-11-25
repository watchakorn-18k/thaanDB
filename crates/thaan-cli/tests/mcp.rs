//! `thaan mcp` end to end: the real binary over stdio, against a SQLite file.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start(store: &std::path::Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_thaan"))
            .arg("mcp")
            .arg("--store")
            .arg(store)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
            next: 1,
        }
    }

    fn send(&mut self, msg: &Json) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Json) -> Json {
        let id = self.next;
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let reply: Json = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id, "reply: {reply}");
        reply
    }

    fn tool(&mut self, name: &str, args: Json) -> Json {
        let r = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let result = &r["result"];
        assert_eq!(result["isError"], false, "{name} failed: {r}");
        let text = result["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.child.kill().ok();
    }
}

#[test]
fn stdio_session_against_sqlite() {
    let dir = std::env::temp_dir().join(format!("thaan-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("shop.db");
    let url = format!("sqlite:{}", db.display());
    let mut c = Client::start(&dir.join("store.json"));

    let init = c.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    c.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    let tools = c.request("tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().len() >= 8);

    // A URL works directly; no connect step.
    let r = c.tool(
        "query",
        json!({ "connection": url, "sql": "create table users(id integer primary key, name text, age int); insert into users(name, age) values ('ann', 31), ('bob', 25), ('cy', 40);" }),
    );
    assert_eq!(r["results"].as_array().unwrap().len(), 2);

    // Save it, then use it by name.
    let saved = c.tool("add_connection", json!({ "url": url, "name": "shop" }));
    assert_eq!(saved["name"], "shop");
    let list = c.tool("list_connections", json!({}));
    assert_eq!(list["connections"][0]["name"], "shop");

    let r = c.tool(
        "query",
        json!({ "connection": "shop", "sql": "select name from users where age > :min order by name", "params": { "min": 30 } }),
    );
    assert_eq!(
        r["results"][0]["rows"],
        json!([{ "name": "ann" }, { "name": "cy" }])
    );

    let r = c.tool(
        "query",
        json!({ "connection": "shop", "sql": "select * from users", "maxRows": 1 }),
    );
    assert_eq!(r["results"][0]["truncated"], true);

    let t = c.tool("list_tables", json!({ "connection": "shop" }));
    assert!(t["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["name"] == "users"));

    let d = c.tool(
        "describe_table",
        json!({ "connection": "shop", "table": "users" }),
    );
    assert_eq!(d["columns"].as_array().unwrap().len(), 3);
    assert!(d["ddl"].as_str().unwrap().contains("CREATE TABLE"));

    let page = c.tool(
        "fetch_rows",
        json!({ "connection": "shop", "table": "users", "where": "age < 35", "sort": ["-age"], "pageSize": 10 }),
    );
    assert_eq!(page["totalRows"], 2);
    assert_eq!(page["rows"][0]["name"], "ann");

    // Anything else goes through run_command.
    c.tool(
        "run_command",
        json!({ "connection": "shop", "command": "table.rename", "params": { "table": { "name": "users" }, "to": "people" } }),
    );
    let r = c.tool(
        "query",
        json!({ "connection": "shop", "sql": "select count(*) as n from people" }),
    );
    assert_eq!(r["results"][0]["rows"][0]["n"], 3);

    // Errors come back as tool results the agent can read.
    let r = c.request(
        "tools/call",
        json!({ "name": "query", "arguments": { "connection": "shop", "sql": "select * from nope" } }),
    );
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("nope"), "{text}");

    c.tool("disconnect", json!({ "connection": "shop" }));
    drop(c);
    std::fs::remove_dir_all(&dir).ok();
}
