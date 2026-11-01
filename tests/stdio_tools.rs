//! Runs the real binary over stdio (as Claude does) against a mocked Kite API.

mod common;

use common::{INSTRUMENTS_CSV, Server, ok};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

#[tokio::test]
async fn permission_error_explains_plan_limit() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/quote/ltp"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "status": "error", "error_type": "PermissionException",
            "message": "Insufficient permission for that call."
        })))
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;
    let err = s
        .call_err("get_ltp", json!({"instruments": ["NSE:INFY"]}))
        .await;
    assert!(err.contains("paid"), "{err}");
    // A permission error must not log the user out.
    let status = s.call("session_status", json!({})).await;
    assert_eq!(status["logged_in"], true);
}
