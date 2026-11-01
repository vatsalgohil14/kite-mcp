//! End-to-end tests for trading, safety, GTT, alerts, margins, mutual funds, the live
//! ticker and summary tools — the real binary over stdio against a mocked Kite API.

mod common;

use common::{INSTRUMENTS_CSV, Server, ok};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mock with the instrument dump, LTP denied (free plan) and a margins response.
async fn base_mock() -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_string(INSTRUMENTS_CSV))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/quote/ltp"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "status": "error", "error_type": "PermissionException",
            "message": "Insufficient permission for that call."
        })))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/margins/orders"))
        .respond_with(ok(
            json!([{ "total": 15000.0, "charges": { "total": 17.5 } }]),
        ))
        .mount(&mock)
        .await;
    mock
}

fn tool_names(list: &Value) -> Vec<String> {
    list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn limit_buy(qty: i64, price: f64) -> Value {
    json!({
        "exchange": "NSE", "tradingsymbol": "INFY", "transaction_type": "BUY",
        "order_type": "LIMIT", "quantity": qty, "product": "CNC", "price": price
    })
}

#[tokio::test]
async fn place_order_needs_confirmation_then_executes_once() {
    let mock = base_mock().await;
    Mock::given(method("POST"))
        .and(path("/orders/regular"))
        .and(body_string_contains("tradingsymbol=INFY"))
        .and(body_string_contains("price=1500"))
        .and(body_string_contains("tag=kitemcp"))
        .respond_with(ok(json!({ "order_id": "151220000000000" })))
        .expect(1)
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;

    let preview = s.call("place_order", limit_buy(10, 1500.0)).await;
    assert_eq!(preview["status"], "needs_confirmation");
    assert!(
        preview["summary"]
            .as_str()
            .unwrap()
            .contains("BUY 10 NSE:INFY LIMIT CNC @ ₹1500")
    );
    assert_eq!(preview["preview"]["estimated_value"], 15000.0);
    assert_eq!(preview["preview"]["margin_and_charges"]["total"], 15000.0);

    let pending = s.call("list_pending_actions", json!({})).await;
    assert_eq!(pending.as_array().unwrap().len(), 1);

    let token = preview["confirm_token"].as_str().unwrap();
    let done = s
        .call("confirm_action", json!({ "confirm_token": token }))
        .await;
    assert_eq!(done["status"], "done");
    assert_eq!(done["result"]["order_id"], "151220000000000");

    let again = s
        .call_err("confirm_action", json!({ "confirm_token": token }))
        .await;
    assert!(again.contains("already used"));

    let audit = s.call("get_audit_log", json!({})).await;
    assert_eq!(audit[0]["action"], "place_order");
    assert_eq!(audit[0]["outcome"], "ok");
    let settings = s.call("get_safety_settings", json!({})).await;
    assert!(settings.get("max_order_value").is_none());
}

#[tokio::test]
async fn no_amount_limits_but_invalid_orders_are_rejected() {
    let mock = base_mock().await;
    Mock::given(method("POST"))
        .and(path("/orders/regular"))
        .respond_with(ok(json!({ "order_id": "1" })))
        .expect(0)
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;

    // Large orders are not capped: they go to the normal preview.
    let big = s.call("place_order", limit_buy(500, 1500.0)).await;
    assert_eq!(big["status"], "needs_confirmation");
    assert_eq!(big["preview"]["estimated_value"], 750000.0);

    let mut market = limit_buy(1, 0.0);
    market["order_type"] = "MARKET".into();
    market.as_object_mut().unwrap().remove("price");
    // MARKET orders work without a live price.
    let m = s.call("place_order", market).await;
    assert_eq!(m["status"], "needs_confirmation");

    let err = s
        .call_err(
            "place_order",
            json!({ "exchange": "NFO", "tradingsymbol": "NIFTY26NOV24000CE", "transaction_type": "BUY",
                    "order_type": "LIMIT", "quantity": 50, "product": "NRML", "price": 100.0 }),
        )
        .await;
    assert!(err.contains("lot size 75"), "{err}");

    let mut unknown = limit_buy(1, 10.0);
    unknown["tradingsymbol"] = "NOPE".into();
    assert!(
        s.call_err("place_order", unknown)
            .await
            .contains("search_instruments")
    );

    let mut no_price = limit_buy(1, 10.0);
    no_price.as_object_mut().unwrap().remove("price");
    assert!(s.call_err("place_order", no_price).await.contains("price"));
}

#[tokio::test]
async fn read_only_mode_hides_mutating_tools() {
    let mock = base_mock().await;
    let mut s = Server::start_with(&mock.uri(), "[safety]\nread_only = true").await;
    let names = tool_names(&s.request("tools/list", json!({})).await);
    for hidden in [
        "place_order",
        "modify_order",
        "cancel_order",
        "convert_position",
        "confirm_action",
        "place_gtt",
        "modify_gtt",
        "delete_gtt",
        "create_alert",
        "delete_alerts",
    ] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} should be hidden"
        );
    }
    for shown in [
        "get_gtts",
        "get_alerts",
        "get_order_margins",
        "get_safety_settings",
        "get_holdings",
    ] {
        assert!(names.contains(&shown.to_string()), "{shown} missing");
    }
}

#[tokio::test]
async fn dry_run_without_confirmation_never_sends() {
    let mock = base_mock().await;
    Mock::given(method("POST"))
        .and(path("/orders/regular"))
        .respond_with(ok(json!({ "order_id": "1" })))
        .expect(0)
        .mount(&mock)
        .await;
    let mut s = Server::start_with(
        &mock.uri(),
        "[safety]\ndry_run = true\nrequire_confirmation = false",
    )
    .await;
    let r = s.call("place_order", limit_buy(1, 1500.0)).await;
    assert_eq!(r["status"], "dry_run");
    let audit = s.call("get_audit_log", json!({})).await;
    assert_eq!(audit[0]["outcome"], "dry_run");
}

#[tokio::test]
async fn modify_and_cancel_open_orders() {
    let mock = base_mock().await;
    let open = json!([{
        "order_id": "555", "status": "OPEN", "variety": "regular", "exchange": "NSE",
        "tradingsymbol": "SBIN", "transaction_type": "BUY", "order_type": "LIMIT",
        "quantity": 5, "pending_quantity": 5, "price": 800.0, "product": "CNC", "validity": "DAY"
    }]);
    Mock::given(method("GET"))
        .and(path("/orders/555"))
        .respond_with(ok(open))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/orders/556"))
        .respond_with(ok(json!([{ "order_id": "556", "status": "COMPLETE" }])))
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/orders/regular/555"))
        .and(body_string_contains("price=790"))
        .and(body_string_contains("quantity=5"))
        .respond_with(ok(json!({ "order_id": "555" })))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/orders/regular/555"))
        .respond_with(ok(json!({ "order_id": "555" })))
        .expect(1)
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;

    let p = s
        .call("modify_order", json!({ "order_id": "555", "price": 790.0 }))
        .await;
    assert!(p["summary"].as_str().unwrap().contains("MODIFY order 555"));
    s.call(
        "confirm_action",
        json!({ "confirm_token": p["confirm_token"] }),
    )
    .await;

    let p = s.call("cancel_order", json!({ "order_id": "555" })).await;
    assert!(
        p["summary"]
            .as_str()
            .unwrap()
            .starts_with("CANCEL order 555")
    );
    let done = s
        .call(
            "confirm_action",
            json!({ "confirm_token": p["confirm_token"] }),
        )
        .await;
    assert_eq!(done["result"]["order_id"], "555");

    let err = s
        .call_err("cancel_order", json!({ "order_id": "556" }))
        .await;
    assert!(err.contains("COMPLETE"));
}

#[tokio::test]
async fn gtt_place_validate_and_delete() {
    let mock = base_mock().await;
    Mock::given(method("POST"))
        .and(path("/gtt/triggers"))
        .and(body_string_contains("type=two-leg"))
        .respond_with(ok(json!({ "trigger_id": 42 })))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/gtt/triggers/42"))
        .respond_with(ok(json!({ "id": 42, "type": "two-leg", "status": "active",
            "condition": { "exchange": "NSE", "tradingsymbol": "INFY" } })))
        .mount(&mock)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/gtt/triggers/42"))
        .respond_with(ok(json!({ "trigger_id": 42 })))
        .expect(1)
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;

    let gtt = json!({
        "type": "two-leg", "exchange": "NSE", "tradingsymbol": "INFY", "last_price": 1500.0,
        "trigger_values": [1400.0, 1700.0],
        "orders": [
            { "transaction_type": "SELL", "quantity": 5, "price": 1395.0, "product": "CNC" },
            { "transaction_type": "SELL", "quantity": 5, "price": 1705.0, "product": "CNC" }
        ]
    });
    let p = s.call("place_gtt", gtt.clone()).await;
    assert!(
        p["summary"]
            .as_str()
            .unwrap()
            .contains("at ₹1400 SELL 5 @ ₹1395")
    );
    let done = s
        .call(
            "confirm_action",
            json!({ "confirm_token": p["confirm_token"] }),
        )
        .await;
    assert_eq!(done["result"]["trigger_id"], 42);

    let mut bad = gtt.clone();
    bad["trigger_values"] = json!([1700.0, 1400.0]);
    assert!(s.call_err("place_gtt", bad).await.contains("ascending"));
    let mut no_ltp = gtt;
    no_ltp.as_object_mut().unwrap().remove("last_price");
    assert!(s.call_err("place_gtt", no_ltp).await.contains("last_price"));

    let p = s.call("delete_gtt", json!({ "trigger_id": 42 })).await;
    assert!(p["summary"].as_str().unwrap().contains("DELETE GTT 42"));
    s.call(
        "confirm_action",
        json!({ "confirm_token": p["confirm_token"] }),
    )
    .await;
}

#[tokio::test]
async fn alerts_simple_and_ato() {
    let mock = base_mock().await;
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .and(body_string_contains("type=simple"))
        .respond_with(ok(json!({ "uuid": "u-1" })))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .and(body_string_contains("type=ato"))
        .respond_with(ok(json!({ "uuid": "u-2" })))
        .expect(1)
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;

    let simple = s
        .call(
            "create_alert",
            json!({ "name": "nifty 27k", "exchange": "INDICES", "tradingsymbol": "NIFTY 50",
                    "operator": ">=", "value": 27000.0 }),
        )
        .await;
    assert_eq!(simple["uuid"], "u-1");

    let ato = s
        .call(
            "create_alert",
            json!({ "name": "buy infy dip", "exchange": "NSE", "tradingsymbol": "INFY",
                    "operator": "<=", "value": 1400.0,
                    "order": { "transaction_type": "BUY", "quantity": 5, "product": "CNC",
                               "order_type": "LIMIT", "price": 1400.0 } }),
        )
        .await;
    assert_eq!(ato["status"], "needs_confirmation");
    let done = s
        .call(
            "confirm_action",
            json!({ "confirm_token": ato["confirm_token"] }),
        )
        .await;
    assert_eq!(done["result"]["uuid"], "u-2");

    let err = s
        .call_err(
            "create_alert",
            json!({ "name": "x", "exchange": "NSE", "tradingsymbol": "INFY", "operator": "=>", "value": 1.0 }),
        )
        .await;
    assert!(err.contains("operator"));
}

#[tokio::test]
async fn margins_charges_and_mutual_funds() {
    let mock = base_mock().await;
    Mock::given(method("POST"))
        .and(path("/margins/basket"))
        .and(query_param("consider_positions", "true"))
        .respond_with(ok(
            json!({ "initial": { "total": 120000.0 }, "final": { "total": 30000.0 } }),
        ))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/charges/orders"))
        .and(body_string_contains("\"average_price\":560"))
        .respond_with(ok(json!([{ "charges": { "total": 0.6 } }])))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/mf/holdings"))
        .respond_with(ok(json!([
            { "fund": "PARAG PARIKH FLEXI CAP FUND - DIRECT PLAN", "tradingsymbol": "INF879O01027",
              "quantity": 100.0, "average_price": 50.0, "last_price": 60.0, "last_price_date": "2026-10-31" }
        ])))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/mf/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "tradingsymbol,amc,name,purchase_allowed,redemption_allowed,minimum_purchase_amount,purchase_amount_multiplier,minimum_additional_purchase_amount,minimum_redemption_quantity,redemption_quantity_multiplier,dividend_type,scheme_type,plan,settlement_type,last_price,last_price_date\n\
             INF879O01027,PPFAS_MF,Parag Parikh Flexi Cap Fund - Direct Plan,1,1,1000,1,1000,0.001,0.001,growth,equity,direct,T3,60.1,2026-10-31\n\
             INF879O01019,PPFAS_MF,Parag Parikh Flexi Cap Fund - Regular Plan,1,1,1000,1,1000,0.001,0.001,growth,equity,regular,T3,55.2,2026-10-31\n",
        ))
        .expect(1)
        .mount(&mock)
        .await;
    let mut s = Server::start(&mock.uri()).await;

    let leg = |side: &str, sym: &str| {
        json!({ "exchange": "NFO", "tradingsymbol": sym, "transaction_type": side,
                "product": "NRML", "order_type": "MARKET", "quantity": 75 })
    };
    let basket = s
        .call(
            "get_basket_margins",
            json!({ "orders": [leg("SELL", "NIFTY26NOV24000CE"), leg("BUY", "NIFTY26NOV24100CE")] }),
        )
        .await;
    assert_eq!(basket["final"]["total"], 30000.0);

    let charges = s
        .call(
            "get_order_charges",
            json!({ "orders": [{ "exchange": "NSE", "tradingsymbol": "SBIN", "transaction_type": "BUY",
                    "product": "CNC", "order_type": "MARKET", "quantity": 1, "average_price": 560.0 }] }),
        )
        .await;
    assert_eq!(charges[0]["charges"]["total"], 0.6);
    assert!(
        s.call_err("get_order_margins", json!({ "orders": [] }))
            .await
            .contains("at least one")
    );

    let mf = s.call("get_mf_holdings", json!({})).await;
    assert_eq!(mf["summary"]["invested"], 5000.0);
    assert_eq!(mf["summary"]["pnl_pct"], 20.0);

    let found = s
        .call(
            "search_mf_instruments",
            json!({ "query": "parag flexi", "plan": "direct" }),
        )
        .await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["tradingsymbol"], "INF879O01027");
    // Second search is served from the cache (mock expects one download).
    s.call("search_mf_instruments", json!({ "query": "parag" }))
        .await;
}

#[tokio::test]
async fn portfolio_summary_combines_sources() {
    let mock = base_mock().await;
    Mock::given(method("GET"))
        .and(path("/portfolio/holdings"))
        .respond_with(ok(json!([
            { "tradingsymbol": "INFY", "quantity": 10, "average_price": 1500.0, "last_price": 1650.0,
              "day_change": 10.0, "day_change_percentage": 0.61 },
            { "tradingsymbol": "SBIN", "quantity": 20, "average_price": 800.0, "last_price": 760.0,
              "day_change": -4.0, "day_change_percentage": -0.52 }
        ])))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/portfolio/positions"))
        .respond_with(ok(
            json!({ "net": [{ "tradingsymbol": "X", "quantity": 1, "pnl": 50.0 }], "day": [] }),
        ))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/margins"))
        .respond_with(ok(json!({ "equity": { "enabled": true, "net": 1234.5 } })))
        .mount(&mock)
        .await;
    // MF holdings fail: the summary still renders the rest.
    let mut s = Server::start(&mock.uri()).await;
    let sum = s.call("portfolio_summary", json!({})).await;
    assert_eq!(sum["equity_holdings"]["pnl"], 700.0);
    assert_eq!(
        sum["equity_holdings"]["top_gainers_today"][0]["symbol"],
        "INFY"
    );
    assert_eq!(
        sum["equity_holdings"]["top_losers_today"][0]["symbol"],
        "SBIN"
    );
    assert_eq!(sum["positions"]["open"], 1);
    assert_eq!(sum["funds"]["equity_available"], 1234.5);
    assert!(sum["mutual_funds"]["unavailable"].is_string());

    let status = s.call("market_status", json!({})).await;
    assert!(status["equity_and_fno"]["status"].is_string());
}

#[tokio::test]
async fn live_ticker_stops_on_forbidden() {
    let mock = base_mock().await;
    // A plain HTTP server answering the WebSocket upgrade with 403, like Kite on the free plan.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n")
                .await;
        }
    });
    let mut s = Server::start_with(&mock.uri(), &format!("ticker_url = \"ws://{addr}\"")).await;
    s.call("subscribe_ticks", json!({ "instruments": ["NSE:INFY"] }))
        .await;
    let mut status = Value::Null;
    for _ in 0..30 {
        status = s.call("ticker_status", json!({})).await;
        if status["status"]["running"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(status["status"]["running"], false, "{status}");
    assert_eq!(status["status"]["subscriptions"], 0);
    assert!(
        status["status"]["last_error"]
            .as_str()
            .unwrap()
            .contains("paid")
    );
}

/// Builds a Kite binary frame with one INFY quote packet.
fn quote_frame() -> Vec<u8> {
    let fields: [i32; 11] = [
        408065, 141295, 5, 141247, 7_360_198, 10, 20, 139600, 142175, 139555, 138965,
    ];
    let packet: Vec<u8> = fields.iter().flat_map(|f| f.to_be_bytes()).collect();
    let mut frame = 1u16.to_be_bytes().to_vec();
    frame.extend((packet.len() as u16).to_be_bytes());
    frame.extend(packet);
    frame
}

#[tokio::test]
async fn live_ticker_streams_ticks_and_order_updates() {
    let mock = base_mock().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (got_tx, mut got_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        // Wait for the subscribe + mode messages, then stream data.
        for _ in 0..2 {
            if let Some(Ok(Message::Text(t))) = ws.next().await {
                let _ = got_tx.send(t.to_string());
            }
        }
        ws.send(Message::Binary(quote_frame().into()))
            .await
            .unwrap();
        ws.send(Message::Text(
            json!({ "type": "order", "data": { "order_id": "9", "status": "COMPLETE" } })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        // Keep the connection open until the client goes away.
        while ws.next().await.is_some() {}
    });

    let mut s = Server::start_with(&mock.uri(), &format!("ticker_url = \"ws://{addr}\"")).await;
    let sub = s
        .call(
            "subscribe_ticks",
            json!({ "instruments": ["infy"], "mode": "quote" }),
        )
        .await;
    assert_eq!(sub["subscribed"][0], "NSE:INFY");

    let mut ticks = Value::Null;
    for _ in 0..50 {
        ticks = s.call("get_live_ticks", json!({})).await;
        if ticks["ticks"]["NSE:INFY"].is_object() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let infy = &ticks["ticks"]["NSE:INFY"];
    assert_eq!(infy["last_price"], 1412.95, "{ticks}");
    assert_eq!(infy["volume"], 7_360_198);
    assert_eq!(ticks["connected"], true);

    let subscribe = got_rx.recv().await.unwrap();
    assert!(
        subscribe.contains("\"subscribe\"") && subscribe.contains("408065"),
        "{subscribe}"
    );
    let mode = got_rx.recv().await.unwrap();
    assert!(mode.contains("\"quote\""), "{mode}");

    let updates = s.call("get_order_updates", json!({})).await;
    assert_eq!(updates[0]["data"]["order_id"], "9");

    let status = s.call("ticker_status", json!({})).await;
    assert_eq!(status["subscriptions"][0]["instrument"], "NSE:INFY");
    let un = s.call("unsubscribe_ticks", json!({})).await;
    assert_eq!(un["unsubscribed"], 1);
}
