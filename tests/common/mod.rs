//! Shared harness: runs the real binary over stdio (as Claude does) against a mocked
//! Kite API.
#![allow(dead_code)]

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use wiremock::ResponseTemplate;

pub const INSTRUMENTS_CSV: &str = "\
instrument_token,exchange_token,tradingsymbol,name,last_price,expiry,strike,tick_size,lot_size,instrument_type,segment,exchange
408065,1594,INFY,INFOSYS,0,,0,0.05,1,EQ,NSE,NSE
779521,3045,SBIN,STATE BANK OF INDIA,0,,0,0.05,1,EQ,NSE,NSE
13240578,51721,NIFTY26NOV24000CE,NIFTY,0,2026-11-24,24000,0.05,75,CE,NFO-OPT,NFO
";

pub fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "status": "success", "data": data }))
}

pub struct Server {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    pub home: tempfile::TempDir,
}

impl Server {
    pub async fn start(api_base: &str) -> Self {
        Self::start_with(api_base, "").await
    }

    /// Starts the server with extra TOML appended to its config (e.g. `[safety]`).
    pub async fn start_with(api_base: &str, extra_config: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!("api_key = \"key\"\napi_secret = \"secret\"\napi_base = \"{api_base}\"\n{extra_config}\n"),
        )
        .unwrap();
        let now = Utc::now();
        std::fs::write(
            home.path().join("session.json"),
            json!({
                "user_id": "AB1234", "user_name": "Test", "access_token": "acc",
                "created_at": now, "expires_at": now + Duration::hours(6),
            })
            .to_string(),
        )
        .unwrap();

        let mut child = Command::new(env!("CARGO_BIN_EXE_kite-mcp"))
            .env("KITE_MCP_HOME", home.path())
            .env_remove("KITE_API_KEY")
            .env_remove("KITE_API_SECRET")
            .env("KITE_MCP_LOG", "warn")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut s = Self {
            _child: child,
            stdin,
            lines,
            next_id: 1,
            home,
        };
        s.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}),
        )
        .await;
        s.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        s
    }

    pub async fn send(&mut self, msg: Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(10), self.lines.next_line())
                    .await
                    .expect("timed out waiting for response")
                    .unwrap()
                    .expect("server closed stdout");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                return msg["result"].clone();
            }
        }
    }

    /// Calls a tool, asserts success, and returns its parsed JSON output.
    pub async fn call(&mut self, name: &str, args: Value) -> Value {
        let r = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let text = r["content"][0]["text"].as_str().unwrap_or_default();
        assert_ne!(r["isError"], true, "{name} failed: {text}");
        serde_json::from_str(text).unwrap()
    }

    pub async fn call_err(&mut self, name: &str, args: Value) -> String {
        let r = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        assert_eq!(r["isError"], true, "{name} unexpectedly succeeded");
        r["content"][0]["text"].as_str().unwrap().to_string()
    }
}
