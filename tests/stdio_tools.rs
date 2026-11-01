//! Runs the real binary over stdio (as Claude does) against a mocked Kite API.

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const INSTRUMENTS_CSV: &str = "\
instrument_token,exchange_token,tradingsymbol,name,last_price,expiry,strike,tick_size,lot_size,instrument_type,segment,exchange
408065,1594,INFY,INFOSYS,0,,0,0.05,1,EQ,NSE,NSE
779521,3045,SBIN,STATE BANK OF INDIA,0,,0,0.05,1,EQ,NSE,NSE
13240578,51721,NIFTY26NOV24000CE,NIFTY,0,2026-11-24,24000,0.05,75,CE,NFO-OPT,NFO
";

fn ok(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "status": "success", "data": data }))
}

struct Server {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    _home: tempfile::TempDir,
}

impl Server {
    async fn start(api_base: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!("api_key = \"key\"\napi_secret = \"secret\"\napi_base = \"{api_base}\"\n"),
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

        let mut child = Command::new(env!("CARGO_BIN_EXE_personal-kite-mcp"))
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
            _home: home,
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

    async fn send(&mut self, msg: Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
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
    async fn call(&mut self, name: &str, args: Value) -> Value {
        let r = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let text = r["content"][0]["text"].as_str().unwrap_or_default();
        assert_ne!(r["isError"], true, "{name} failed: {text}");
        serde_json::from_str(text).unwrap()
    }

    async fn call_err(&mut self, name: &str, args: Value) -> String {
        let r = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        assert_eq!(r["isError"], true, "{name} unexpectedly succeeded");
        r["content"][0]["text"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn lists_all_phase2_tools() {
    let mock = MockServer::start().await;
    let mut s = Server::start(&mock.uri()).await;
    let tools = s.request("tools/list", json!({})).await;
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "login",
        "session_status",
        "get_profile",
        "get_margins",
        "get_holdings",
        "get_positions",
        "get_auctions",
        "get_orders",
        "get_order_history",
        "get_trades",
        "get_order_trades",
        "search_instruments",
        "get_instrument",
        "get_quote",
        "get_ohlc",
        "get_ltp",
        "get_historical_data",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
}

#[tokio::test]
async fn portfolio_and_orders() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/portfolio/holdings"))
        .respond_with(ok(json!([
            {"tradingsymbol": "INFY", "exchange": "NSE", "quantity": 10, "average_price": 1500.0,
             "last_price": 1650.0, "day_change": 10.0, "day_change_percentage": 0.61},
            {"tradingsymbol": "SBIN", "exchange": "NSE", "quantity": 20, "average_price": 800.0,
             "last_price": 760.0, "day_change": -4.0, "day_change_percentage": -0.52}
        ])))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/portfolio/positions"))
        .respond_with(ok(json!({
            "net": [
                {"tradingsymbol": "NIFTY26NOV24000CE", "exchange": "NFO", "product": "NRML",
                 "quantity": 75, "pnl": 1200.5, "m2m": 300.0},
                {"tradingsymbol": "SBIN", "exchange": "NSE", "product": "MIS", "quantity": 0, "pnl": -50.0}
            ],
            "day": []
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/orders"))
        .respond_with(ok(json!([
            {"order_id": "1", "status": "COMPLETE", "tradingsymbol": "INFY", "transaction_type": "BUY", "quantity": 1},
            {"order_id": "2", "status": "OPEN", "tradingsymbol": "SBIN", "transaction_type": "SELL", "quantity": 5, "price": 900.0}
        ])))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/margins"))
        .respond_with(ok(json!({"equity": {"enabled": true, "net": 99725.05,
            "available": {"cash": 245431.6}, "utilised": {"debits": 145706.55}}})))
        .mount(&mock)
        .await;

    let mut s = Server::start(&mock.uri()).await;

    let h = s.call("get_holdings", json!({"sort_by": "pnl"})).await;
    assert_eq!(h["summary"]["count"], 2);
    assert_eq!(h["summary"]["invested"], 31000.0);
    assert_eq!(h["summary"]["total_pnl"], 700.0);
    assert_eq!(h["holdings"][0]["symbol"], "INFY");

    let p = s.call("get_positions", json!({})).await;
    assert_eq!(p["open_count"], 1);
    assert_eq!(p["positions"].as_array().unwrap().len(), 1);
    assert_eq!(p["total_pnl"], 1150.5);

    let o = s.call("get_orders", json!({"status": "open"})).await;
    assert_eq!(o["total"], 2);
    assert_eq!(o["orders"][0]["order_id"], "2");

    let m = s.call("get_margins", json!({})).await;
    assert_eq!(m["equity"]["net"], 99725.05);
    assert!(
        s.call_err("get_margins", json!({"segment": "fx"}))
            .await
            .contains("equity")
    );
}

#[tokio::test]
async fn instruments_quotes_and_history() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_string(INSTRUMENTS_CSV))
        .expect(1) // cached after the first call
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/quote"))
        .and(query_param("i", "NSE:INFY"))
        .respond_with(ok(json!({"NSE:INFY": {
            "instrument_token": 408065, "last_price": 1412.95, "volume": 7360198,
            "ohlc": {"open": 1396, "high": 1421.75, "low": 1395.55, "close": 1389.65},
            "depth": {"buy": [{"price": 1412.9, "quantity": 10, "orders": 1}], "sell": []}
        }})))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/quote/ltp"))
        .respond_with(ok(
            json!({"NSE:INFY": {"instrument_token": 408065, "last_price": 1074.35}}),
        ))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/instruments/historical/408065/minute"))
        .respond_with(ok(json!({"candles": [
            ["2026-01-05T09:15:00+0530", 1704.5, 1705, 1699.25, 1702.8, 2499],
            ["2026-01-05T09:16:00+0530", 1702, 1702, 1698.15, 1698.15, 1271]
        ]})))
        .expect(4) // 181 days at 60 days per request
        .mount(&mock)
        .await;

    let mut s = Server::start(&mock.uri()).await;

    let found = s
        .call(
            "search_instruments",
            json!({"query": "nifty", "instrument_type": "CE"}),
        )
        .await;
    assert_eq!(found[0]["key"], "NFO:NIFTY26NOV24000CE");
    assert_eq!(found[0]["lot_size"], 75);

    let inst = s
        .call("get_instrument", json!({"instrument": "SBIN"}))
        .await;
    assert_eq!(inst["instrument_token"], 779521);

    let q = s.call("get_quote", json!({"instruments": ["infy"]})).await;
    assert_eq!(q["NSE:INFY"]["last_price"], 1412.95);
    assert!(q["NSE:INFY"].get("depth").is_none());
    let q = s
        .call(
            "get_quote",
            json!({"instruments": ["NSE:INFY"], "include_depth": true}),
        )
        .await;
    assert_eq!(q["NSE:INFY"]["depth"]["buy"][0]["price"], 1412.9);

    let ltp = s
        .call("get_ltp", json!({"instruments": ["NSE:INFY"]}))
        .await;
    assert_eq!(ltp["NSE:INFY"], 1074.35);

    let hist = s
        .call(
            "get_historical_data",
            json!({"instrument": "NSE:INFY", "interval": "minute",
                   "from": "2026-01-01", "to": "2026-06-30", "max_candles": 5}),
        )
        .await;
    assert_eq!(hist["total_candles"], 8);
    assert_eq!(hist["returned"], 5);
    assert_eq!(hist["truncated_to_latest"], true);
    assert_eq!(hist["columns"][5], "volume");

    let err = s
        .call_err(
            "get_historical_data",
            json!({"instrument": "NSE:INFY", "interval": "2minute", "from": "2026-01-01"}),
        )
        .await;
    assert!(err.contains("unknown interval"));
    let err = s
        .call_err("get_instrument", json!({"instrument": "NSE:NOPE"}))
        .await;
    assert!(err.contains("search_instruments"));
}
