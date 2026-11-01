//! Price alerts. `simple` alerts only notify; `ato` (alert-triggers-order) alerts place an
//! order when triggered, so they go through the safety limits and confirmation.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{KiteError, Result};
use crate::kite::api::trading::Form;
use crate::mcp::server::{KiteMcp, ToolResult};
use crate::mcp::tools::trading::value_form;
use crate::safety::{OrderCheck, Outcome};

const OPERATORS: &[&str] = &["<=", ">=", "<", ">", "=="];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AlertOrder {
    /// BUY or SELL.
    pub transaction_type: String,
    pub quantity: i64,
    /// CNC, MIS, NRML.
    pub product: String,
    /// MARKET or LIMIT.
    pub order_type: String,
    /// Limit price (LIMIT orders).
    #[serde(default)]
    pub price: Option<f64>,
    /// Exchange to place the order on; defaults to the alert's exchange.
    #[serde(default)]
    pub exchange: Option<String>,
    /// Tradingsymbol to trade; defaults to the alert's instrument.
    #[serde(default)]
    pub tradingsymbol: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AlertArgs {
    pub name: String,
    /// Exchange of the watched instrument: NSE, BSE, NFO, CDS, BCD, MCX, or INDICES.
    pub exchange: String,
    /// Watched instrument, e.g. INFY or "NIFTY 50".
    pub tradingsymbol: String,
    /// One of <=, >=, <, >, ==.
    pub operator: String,
    /// Price to compare the last traded price against.
    pub value: f64,
    /// Attribute to watch (default LastTradedPrice).
    #[serde(default)]
    pub attribute: Option<String>,
    /// If set, creates an ATO alert that places this order when triggered.
    #[serde(default)]
    pub order: Option<AlertOrder>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ModifyAlertArgs {
    pub uuid: String,
    pub name: String,
    pub exchange: String,
    pub tradingsymbol: String,
    pub operator: String,
    pub value: f64,
    #[serde(default)]
    pub attribute: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AlertsListArgs {
    /// enabled, disabled or deleted.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AlertIdArgs {
    pub uuid: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteAlertsArgs {
    pub uuids: Vec<String>,
}

fn base_form(
    name: &str,
    exchange: &str,
    symbol: &str,
    operator: &str,
    value: f64,
    attr: Option<&str>,
) -> Result<serde_json::Map<String, Value>> {
    let op = operator.trim();
    if !OPERATORS.contains(&op) {
        return Err(KiteError::InvalidInput(format!(
            "operator must be one of {}",
            OPERATORS.join(" ")
        )));
    }
    let mut m = serde_json::Map::new();
    m.insert("name".into(), name.into());
    m.insert(
        "lhs_exchange".into(),
        exchange.trim().to_ascii_uppercase().into(),
    );
    m.insert(
        "lhs_tradingsymbol".into(),
        symbol.trim().to_ascii_uppercase().into(),
    );
    m.insert(
        "lhs_attribute".into(),
        attr.unwrap_or("LastTradedPrice").into(),
    );
    m.insert("operator".into(), op.into());
    m.insert("rhs_type".into(), "constant".into());
    m.insert("rhs_constant".into(), value.to_string().into());
    m.insert("type".into(), "simple".into());
    Ok(m)
}

#[tool_router(router = alerts_read_router, vis = "pub(crate)")]
impl KiteMcp {
    /// List price alerts, optionally filtered by status, with pagination.
    #[tool(annotations(title = "Get alerts", read_only_hint = true, open_world_hint = true))]
    async fn get_alerts(&self, Parameters(args): Parameters<AlertsListArgs>) -> ToolResult {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(s) = args.status {
            q.push(("status", s));
        }
        if let Some(p) = args.page {
            q.push(("page", p.to_string()));
        }
        if let Some(p) = args.page_size {
            q.push(("page_size", p.to_string()));
        }
        let result = self.client.alerts(&q).await;
        self.respond(result)
    }

    /// Get one alert.
    #[tool(annotations(title = "Get alert", read_only_hint = true, open_world_hint = true))]
    async fn get_alert(&self, Parameters(args): Parameters<AlertIdArgs>) -> ToolResult {
        let result = self.client.alert(args.uuid.trim()).await;
        self.respond(result)
    }

    /// Get the trigger history of one alert.
    #[tool(annotations(
        title = "Get alert history",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_alert_history(&self, Parameters(args): Parameters<AlertIdArgs>) -> ToolResult {
        let result = self.client.alert_history(args.uuid.trim()).await;
        self.respond(result)
    }
}

#[tool_router(router = alerts_write_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Create a price alert (e.g. "NIFTY 50 >= 27000"). Add `order` to make it an ATO alert
    /// that places an order when triggered (goes through safety limits and confirmation).
    #[tool(annotations(
        title = "Create alert",
        destructive_hint = false,
        open_world_hint = true
    ))]
    async fn create_alert(&self, Parameters(args): Parameters<AlertArgs>) -> ToolResult {
        let result = self.create_alert_impl(args).await;
        self.respond(result)
    }

    /// Change an existing simple alert's condition or name.
    #[tool(annotations(
        title = "Modify alert",
        destructive_hint = false,
        open_world_hint = true
    ))]
    async fn modify_alert(&self, Parameters(args): Parameters<ModifyAlertArgs>) -> ToolResult {
        let result = match base_form(
            &args.name,
            &args.exchange,
            &args.tradingsymbol,
            &args.operator,
            args.value,
            args.attribute.as_deref(),
        ) {
            Ok(m) => {
                let form: Form = value_form(&Value::Object(m.clone()));
                let r = self.client.modify_alert(args.uuid.trim(), &form).await;
                self.audit_simple("modify_alert", &Value::Object(m), &r);
                r
            }
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Delete one or more alerts by uuid.
    #[tool(annotations(
        title = "Delete alerts",
        destructive_hint = true,
        open_world_hint = true
    ))]
    async fn delete_alerts(&self, Parameters(args): Parameters<DeleteAlertsArgs>) -> ToolResult {
        let result = if args.uuids.is_empty() {
            Err(KiteError::InvalidInput("pass at least one uuid".into()))
        } else {
            let r = self.client.delete_alerts(&args.uuids).await;
            self.audit_simple("delete_alerts", &json!(args.uuids), &r);
            r
        };
        self.respond(result)
    }
}

impl KiteMcp {
    fn audit_simple(&self, action: &str, request: &Value, result: &Result<Value>) {
        match result {
            Ok(v) => self.safety.audit.record(action, Outcome::Ok, request, v),
            Err(e) => self.safety.audit.record(
                action,
                Outcome::Error,
                request,
                &json!({ "error": e.to_string() }),
            ),
        }
    }

    async fn create_alert_impl(&self, a: AlertArgs) -> Result<Value> {
        let mut form = base_form(
            &a.name,
            &a.exchange,
            &a.tradingsymbol,
            &a.operator,
            a.value,
            a.attribute.as_deref(),
        )?;
        let Some(o) = a.order else {
            let request = json!({ "form": form });
            if self.safety.config.dry_run {
                return self.execute("create_alert", request).await;
            }
            let r = self
                .client
                .create_alert(&value_form(&request["form"]))
                .await;
            self.audit_simple("create_alert", &request, &r);
            return r;
        };

        // ATO: build the basket and treat it like an order.
        let exchange = o
            .exchange
            .clone()
            .unwrap_or_else(|| a.exchange.clone())
            .trim()
            .to_ascii_uppercase();
        if exchange == "INDICES" {
            return Err(KiteError::InvalidInput(
                "set `order.exchange` and `order.tradingsymbol` to a tradable instrument".into(),
            ));
        }
        let symbol = o
            .tradingsymbol
            .clone()
            .unwrap_or_else(|| a.tradingsymbol.clone())
            .trim()
            .to_ascii_uppercase();
        let side = o.transaction_type.trim().to_ascii_uppercase();
        let product = o.product.trim().to_ascii_uppercase();
        let order_type = o.order_type.trim().to_ascii_uppercase();
        if !matches!(order_type.as_str(), "MARKET" | "LIMIT") {
            return Err(KiteError::InvalidInput(
                "ATO order_type must be MARKET or LIMIT".into(),
            ));
        }
        if order_type == "LIMIT" && o.price.is_none_or(|p| p <= 0.0) {
            return Err(KiteError::InvalidInput(
                "LIMIT ATO orders need a positive `price`".into(),
            ));
        }
        self.safety.check_order(&OrderCheck {
            exchange: &exchange,
            tradingsymbol: &symbol,
            product: &product,
            quantity: o.quantity,
        })?;
        let basket = json!({
            "name": a.name, "type": "alert", "tags": [],
            "items": [{
                "type": "insert", "tradingsymbol": symbol, "exchange": exchange, "weight": 0,
                "params": {
                    "transaction_type": side, "product": product, "order_type": order_type,
                    "validity": "DAY", "validity_ttl": 1, "quantity": o.quantity,
                    "price": o.price.unwrap_or(0.0), "trigger_price": 0, "disclosed_quantity": 0,
                    "last_price": 0, "variety": "regular", "tags": [], "squareoff": 0,
                    "stoploss": 0, "trailing_stoploss": 0, "iceberg_legs": 0,
                    "market_protection": -1
                }
            }]
        });
        form.insert("type".into(), "ato".into());
        form.insert("basket".into(), basket.to_string().into());
        let summary = format!(
            "ATO alert '{}': when {}:{} {} {} -> {side} {} {exchange}:{symbol} {order_type}{} {product}",
            a.name,
            a.exchange.to_ascii_uppercase(),
            a.tradingsymbol.to_ascii_uppercase(),
            a.operator.trim(),
            a.value,
            o.quantity,
            o.price.map(|p| format!(" @ ₹{p}")).unwrap_or_default(),
        );
        let request = json!({ "form": form });
        self.gate("create_alert", request.clone(), summary, request)
            .await
    }
}
