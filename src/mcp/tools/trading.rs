//! Order placement, modification, cancellation and position conversion, all routed
//! through the safety layer (limits, confirmation, dry-run, audit).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{KiteError, Result};
use crate::kite::api::trading::Form;
use crate::kite::models::orders::Order;
use crate::mcp::server::{KiteMcp, ToolResult};
use crate::safety::{OrderCheck, Outcome, PendingAction};

const VARIETIES: &[&str] = &["regular", "amo", "co", "iceberg", "auction"];
const ORDER_TYPES: &[&str] = &["MARKET", "LIMIT", "SL", "SL-M"];
const PRODUCTS: &[&str] = &["CNC", "NRML", "MIS", "MTF"];
const VALIDITIES: &[&str] = &["DAY", "IOC", "TTL"];
const SIDES: &[&str] = &["BUY", "SELL"];

fn one_of(field: &str, value: &str, allowed: &[&str]) -> Result<String> {
    let v = value.trim().to_string();
    let up = if field == "variety" {
        v.to_ascii_lowercase()
    } else {
        v.to_ascii_uppercase()
    };
    if allowed.contains(&up.as_str()) {
        Ok(up)
    } else {
        Err(KiteError::InvalidInput(format!(
            "{field} must be one of {}; got `{value}`",
            allowed.join(", ")
        )))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaceOrderArgs {
    /// regular (default), amo (after market), co (cover), iceberg, auction.
    #[serde(default)]
    pub variety: Option<String>,
    /// NSE, BSE, NFO, BFO, CDS, BCD, MCX.
    pub exchange: String,
    /// Exact tradingsymbol, e.g. INFY or NIFTY26NOV24000CE (see `search_instruments`).
    pub tradingsymbol: String,
    /// BUY or SELL.
    pub transaction_type: String,
    /// MARKET, LIMIT, SL (stop-loss limit) or SL-M (stop-loss market).
    pub order_type: String,
    /// Number of shares/units. For F&O this must be a multiple of the lot size.
    pub quantity: i64,
    /// CNC (delivery), MIS (intraday), NRML (F&O overnight), MTF (margin trading).
    pub product: String,
    /// Limit price. Required for LIMIT and SL.
    #[serde(default)]
    pub price: Option<f64>,
    /// Trigger price. Required for SL and SL-M (and LIMIT cover orders).
    #[serde(default)]
    pub trigger_price: Option<f64>,
    #[serde(default)]
    pub disclosed_quantity: Option<i64>,
    /// DAY (default), IOC or TTL.
    #[serde(default)]
    pub validity: Option<String>,
    /// Minutes, for TTL validity.
    #[serde(default)]
    pub validity_ttl: Option<i64>,
    /// Iceberg: number of legs (2-50).
    #[serde(default)]
    pub iceberg_legs: Option<i64>,
    /// Iceberg: quantity per leg.
    #[serde(default)]
    pub iceberg_quantity: Option<i64>,
    /// Auction orders: the auction number.
    #[serde(default)]
    pub auction_number: Option<String>,
    /// MARKET / SL-M protection %: 0-100, or -1 for automatic (default -1).
    #[serde(default)]
    pub market_protection: Option<f64>,
    /// Split orders above the exchange freeze quantity.
    #[serde(default)]
    pub autoslice: Option<bool>,
    /// Optional tag (alphanumeric, max 20). Defaults to the configured order_tag.
    #[serde(default)]
    pub tag: Option<String>,
}

impl PlaceOrderArgs {
    /// Normalizes enums and checks field combinations.
    fn normalize(mut self) -> Result<Self> {
        self.variety = Some(one_of(
            "variety",
            self.variety.as_deref().unwrap_or("regular"),
            VARIETIES,
        )?);
        self.exchange = self.exchange.trim().to_ascii_uppercase();
        self.tradingsymbol = self.tradingsymbol.trim().to_ascii_uppercase();
        self.transaction_type = one_of("transaction_type", &self.transaction_type, SIDES)?;
        self.order_type = one_of("order_type", &self.order_type, ORDER_TYPES)?;
        self.product = one_of("product", &self.product, PRODUCTS)?;
        self.validity = Some(one_of(
            "validity",
            self.validity.as_deref().unwrap_or("DAY"),
            VALIDITIES,
        )?);
        let needs_price = matches!(self.order_type.as_str(), "LIMIT" | "SL");
        let needs_trigger = matches!(self.order_type.as_str(), "SL" | "SL-M");
        if needs_price && self.price.is_none_or(|p| p <= 0.0) {
            return Err(KiteError::InvalidInput(format!(
                "{} orders need a positive `price`",
                self.order_type
            )));
        }
        if needs_trigger && self.trigger_price.is_none_or(|p| p <= 0.0) {
            return Err(KiteError::InvalidInput(format!(
                "{} orders need a positive `trigger_price`",
                self.order_type
            )));
        }
        if self.validity.as_deref() == Some("TTL") && self.validity_ttl.is_none() {
            return Err(KiteError::InvalidInput(
                "TTL validity needs `validity_ttl` (minutes)".into(),
            ));
        }
        if self.variety.as_deref() == Some("iceberg")
            && (self.iceberg_legs.is_none() || self.iceberg_quantity.is_none())
        {
            return Err(KiteError::InvalidInput(
                "iceberg orders need `iceberg_legs` and `iceberg_quantity`".into(),
            ));
        }
        if self.variety.as_deref() == Some("auction") && self.auction_number.is_none() {
            return Err(KiteError::InvalidInput(
                "auction orders need `auction_number`".into(),
            ));
        }
        if matches!(self.order_type.as_str(), "MARKET" | "SL-M") && self.market_protection.is_none()
        {
            self.market_protection = Some(-1.0);
        }
        Ok(self)
    }

    fn form(&self, default_tag: &str) -> Form {
        let mut f: Form = vec![
            ("exchange", self.exchange.clone()),
            ("tradingsymbol", self.tradingsymbol.clone()),
            ("transaction_type", self.transaction_type.clone()),
            ("order_type", self.order_type.clone()),
            ("quantity", self.quantity.to_string()),
            ("product", self.product.clone()),
            (
                "validity",
                self.validity.clone().unwrap_or_else(|| "DAY".into()),
            ),
        ];
        let mut opt = |k: &'static str, v: Option<String>| {
            if let Some(v) = v {
                f.push((k, v));
            }
        };
        opt("price", self.price.map(|p| p.to_string()));
        opt("trigger_price", self.trigger_price.map(|p| p.to_string()));
        opt(
            "disclosed_quantity",
            self.disclosed_quantity.map(|q| q.to_string()),
        );
        opt("validity_ttl", self.validity_ttl.map(|q| q.to_string()));
        opt("iceberg_legs", self.iceberg_legs.map(|q| q.to_string()));
        opt(
            "iceberg_quantity",
            self.iceberg_quantity.map(|q| q.to_string()),
        );
        opt("auction_number", self.auction_number.clone());
        opt(
            "market_protection",
            self.market_protection.map(|q| q.to_string()),
        );
        opt("autoslice", self.autoslice.map(|b| b.to_string()));
        let tag = self.tag.clone().unwrap_or_else(|| default_tag.to_string());
        opt("tag", (!tag.is_empty()).then_some(tag));
        f
    }

    fn margin_leg(&self) -> Value {
        json!({
            "exchange": self.exchange,
            "tradingsymbol": self.tradingsymbol,
            "transaction_type": self.transaction_type,
            "variety": self.variety.as_deref().unwrap_or("regular"),
            "product": self.product,
            "order_type": self.order_type,
            "quantity": self.quantity,
            "price": self.price.unwrap_or(0.0),
            "trigger_price": self.trigger_price.unwrap_or(0.0),
        })
    }

    fn summary(&self) -> String {
        let px = match (self.price, self.trigger_price) {
            (Some(p), Some(t)) => format!(" @ ₹{p} (trigger ₹{t})"),
            (Some(p), None) => format!(" @ ₹{p}"),
            (None, Some(t)) => format!(" (trigger ₹{t})"),
            (None, None) => String::new(),
        };
        format!(
            "{} {} {}:{} {} {}{} [{} {}]",
            self.transaction_type,
            self.quantity,
            self.exchange,
            self.tradingsymbol,
            self.order_type,
            self.product,
            px,
            self.variety.as_deref().unwrap_or("regular"),
            self.validity.as_deref().unwrap_or("DAY"),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModifyOrderArgs {
    pub order_id: String,
    /// New order type (MARKET, LIMIT, SL, SL-M).
    #[serde(default)]
    pub order_type: Option<String>,
    #[serde(default)]
    pub quantity: Option<i64>,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub trigger_price: Option<f64>,
    #[serde(default)]
    pub disclosed_quantity: Option<i64>,
    /// DAY, IOC or TTL.
    #[serde(default)]
    pub validity: Option<String>,
    /// For cover orders: the parent order id.
    #[serde(default)]
    pub parent_order_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CancelOrderArgs {
    pub order_id: String,
    /// For cover orders: the parent order id.
    #[serde(default)]
    pub parent_order_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConvertPositionArgs {
    pub exchange: String,
    pub tradingsymbol: String,
    /// BUY or SELL (the side of the existing position).
    pub transaction_type: String,
    /// `overnight` or `day`.
    pub position_type: String,
    pub quantity: i64,
    /// Current product, e.g. MIS.
    pub old_product: String,
    /// Target product, e.g. CNC or NRML.
    pub new_product: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuthoriseItem {
    pub isin: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuthoriseArgs {
    /// Holdings (by ISIN) to pre-authorise for selling via CDSL/TPIN.
    pub items: Vec<AuthoriseItem>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfirmArgs {
    /// The `confirm_token` returned by the preview.
    pub confirm_token: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AuditArgs {
    /// Number of most recent entries (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

impl KiteMcp {
    /// Best-effort live price; `None` when market data is not available on the plan.
    pub(crate) async fn try_ltp(&self, exchange: &str, symbol: &str) -> Option<f64> {
        let key = format!("{exchange}:{symbol}");
        match self.client.ltp(std::slice::from_ref(&key)).await {
            Ok(m) => m.get(&key).map(|q| q.last_price),
            Err(e) => {
                tracing::debug!("LTP unavailable for {key}: {e}");
                None
            }
        }
    }

    /// Either queues the action for confirmation or runs it now.
    pub(crate) async fn gate(
        &self,
        action: &str,
        request: Value,
        summary: String,
        preview: Value,
    ) -> Result<Value> {
        if self.safety.config.require_confirmation {
            let token = self.safety.confirmations.add(PendingAction {
                action: action.into(),
                request,
                summary: summary.clone(),
            });
            return Ok(json!({
                "status": "needs_confirmation",
                "confirm_token": token,
                "expires_in_seconds": crate::safety::confirmations_ttl_secs(),
                "action": action,
                "summary": summary,
                "dry_run": self.safety.config.dry_run,
                "preview": preview,
                "next_step": "Show the summary to the user. Only if they explicitly approve, call `confirm_action` with this confirm_token.",
            }));
        }
        self.execute(action, request).await
    }

    /// Executes a validated action, honouring dry-run and recording it in the audit log.
    pub(crate) async fn execute(&self, action: &str, request: Value) -> Result<Value> {
        if self.safety.config.dry_run {
            let resp =
                json!({ "dry_run": true, "message": "Validated but not sent (dry_run = true)." });
            self.safety
                .audit
                .record(action, Outcome::DryRun, &request, &resp);
            return Ok(
                json!({ "status": "dry_run", "action": action, "request": request, "result": resp }),
            );
        }
        let result = self.dispatch(action, &request).await;
        match &result {
            Ok(resp) => self
                .safety
                .audit
                .record(action, Outcome::Ok, &request, resp),
            Err(e) => self.safety.audit.record(
                action,
                Outcome::Error,
                &request,
                &json!({ "error": e.to_string() }),
            ),
        }
        result.map(|resp| json!({ "status": "done", "action": action, "result": resp }))
    }

    async fn dispatch(&self, action: &str, request: &Value) -> Result<Value> {
        let de = |e: serde_json::Error| KiteError::Decode(e.to_string());
        let to_val =
            |v: crate::kite::api::trading::OrderId| serde_json::to_value(v).unwrap_or_default();
        match action {
            "place_order" => {
                let a: PlaceOrderArgs = serde_json::from_value(request.clone()).map_err(de)?;
                let variety = a.variety.clone().unwrap_or_else(|| "regular".into());
                let form = a.form(&self.safety.config.order_tag);
                self.client.place_order(&variety, &form).await.map(to_val)
            }
            "modify_order" => {
                let variety = request["variety"].as_str().unwrap_or("regular").to_string();
                let order_id = request["order_id"].as_str().unwrap_or_default().to_string();
                let form: Form = request["form"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((static_key(k)?, v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                self.client
                    .modify_order(&variety, &order_id, &form)
                    .await
                    .map(to_val)
            }
            "cancel_order" => {
                let variety = request["variety"].as_str().unwrap_or("regular");
                let order_id = request["order_id"].as_str().unwrap_or_default();
                let parent = request["parent_order_id"].as_str();
                self.client
                    .cancel_order(variety, order_id, parent)
                    .await
                    .map(to_val)
            }
            "convert_position" => {
                let a: ConvertPositionArgs = serde_json::from_value(request.clone()).map_err(de)?;
                let form: Form = vec![
                    ("exchange", a.exchange),
                    ("tradingsymbol", a.tradingsymbol),
                    ("transaction_type", a.transaction_type),
                    ("position_type", a.position_type),
                    ("quantity", a.quantity.to_string()),
                    ("old_product", a.old_product),
                    ("new_product", a.new_product),
                ];
                self.client
                    .convert_position(&form)
                    .await
                    .map(|ok| json!({ "converted": ok }))
            }
            "place_gtt" => {
                let kind = request["type"].as_str().unwrap_or("single");
                self.client
                    .place_gtt(kind, &request["condition"], &request["orders"])
                    .await
            }
            "modify_gtt" => {
                let id = request["trigger_id"].as_u64().unwrap_or_default();
                let kind = request["type"].as_str().unwrap_or("single");
                self.client
                    .modify_gtt(id, kind, &request["condition"], &request["orders"])
                    .await
            }
            "delete_gtt" => {
                let id = request["trigger_id"].as_u64().unwrap_or_default();
                self.client.delete_gtt(id).await
            }
            "create_alert" => {
                let form = value_form(&request["form"]);
                self.client.create_alert(&form).await
            }
            other => Err(KiteError::InvalidInput(format!("unknown action `{other}`"))),
        }
    }

    async fn find_order(&self, order_id: &str) -> Result<Order> {
        self.client
            .order_history(order_id.trim())
            .await?
            .pop()
            .ok_or_else(|| KiteError::NotFound(format!("order {order_id}")))
    }

    async fn preview_place(&self, args: PlaceOrderArgs) -> Result<Value> {
        let a = args.normalize()?;
        let idx = self.instruments.get().await?;
        let inst = idx
            .by_symbol(&a.exchange, &a.tradingsymbol)
            .ok_or_else(|| {
                KiteError::NotFound(format!(
                    "{}:{} — use `search_instruments` to find the exact tradingsymbol",
                    a.exchange, a.tradingsymbol
                ))
            })?;
        if inst.lot_size > 1 && a.quantity % inst.lot_size != 0 {
            return Err(KiteError::InvalidInput(format!(
                "quantity {} is not a multiple of the lot size {}",
                a.quantity, inst.lot_size
            )));
        }
        let ltp = self.try_ltp(&a.exchange, &a.tradingsymbol).await;
        let reference_price = a.price.or(a.trigger_price).or(ltp);
        self.safety.check_order(&OrderCheck {
            exchange: &a.exchange,
            tradingsymbol: &a.tradingsymbol,
            product: &a.product,
            quantity: a.quantity,
        })?;

        let margins = self
            .client
            .order_margins(&json!([a.margin_leg()]), false)
            .await
            .map(|m| m.get(0).cloned().unwrap_or(m))
            .unwrap_or_else(|e| json!({ "unavailable": e.to_string() }));
        let preview = json!({
            "order": a.margin_leg(),
            "instrument": { "token": inst.instrument_token, "lot_size": inst.lot_size, "tick_size": inst.tick_size, "name": inst.name },
            "ltp": ltp,
            "estimated_value": reference_price.map(|p| (p * a.quantity as f64 * 100.0).round() / 100.0),
            "margin_and_charges": margins,
        });
        let summary = a.summary();
        let request = serde_json::to_value(&a).map_err(|e| KiteError::Decode(e.to_string()))?;
        self.gate("place_order", request, summary, preview).await
    }

    async fn preview_modify(&self, args: ModifyOrderArgs) -> Result<Value> {
        let current = self.find_order(&args.order_id).await?;
        if !current.is_open() {
            return Err(KiteError::InvalidInput(format!(
                "order {} is {} and can no longer be modified",
                current.order_id, current.status
            )));
        }
        let order_type = match &args.order_type {
            Some(t) => one_of("order_type", t, ORDER_TYPES)?,
            None => current.order_type.clone(),
        };
        let quantity = args.quantity.unwrap_or(current.quantity);
        let price = args
            .price
            .or((current.price > 0.0).then_some(current.price));
        self.safety.check_order(&OrderCheck {
            exchange: &current.exchange,
            tradingsymbol: &current.tradingsymbol,
            product: &current.product,
            quantity,
        })?;

        let mut form = serde_json::Map::new();
        form.insert("order_type".into(), order_type.clone().into());
        form.insert("quantity".into(), quantity.to_string().into());
        if let Some(p) = args.price {
            form.insert("price".into(), p.to_string().into());
        }
        if let Some(t) = args.trigger_price {
            form.insert("trigger_price".into(), t.to_string().into());
        }
        if let Some(d) = args.disclosed_quantity {
            form.insert("disclosed_quantity".into(), d.to_string().into());
        }
        let validity = match &args.validity {
            Some(v) => one_of("validity", v, VALIDITIES)?,
            None => current.validity.clone(),
        };
        if !validity.is_empty() {
            form.insert("validity".into(), validity.into());
        }
        if let Some(p) = &args.parent_order_id {
            form.insert("parent_order_id".into(), p.clone().into());
        }
        if matches!(order_type.as_str(), "MARKET" | "SL-M") {
            form.insert("market_protection".into(), "-1".into());
        }

        let summary = format!(
            "MODIFY order {} ({} {}:{}): {} qty {} -> {} {}{}",
            current.order_id,
            current.transaction_type,
            current.exchange,
            current.tradingsymbol,
            current.order_type,
            current.quantity,
            order_type,
            quantity,
            price.map(|p| format!(" @ ₹{p}")).unwrap_or_default(),
        );
        let request =
            json!({ "variety": current.variety, "order_id": current.order_id, "form": form });
        let preview = json!({ "current": current, "changes": form });
        self.gate("modify_order", request, summary, preview).await
    }

    async fn preview_cancel(&self, args: CancelOrderArgs) -> Result<Value> {
        let current = self.find_order(&args.order_id).await?;
        if !current.is_open() {
            return Err(KiteError::InvalidInput(format!(
                "order {} is {} and cannot be cancelled",
                current.order_id, current.status
            )));
        }
        let summary = format!(
            "CANCEL order {}: {} {} {}:{} {} (pending {})",
            current.order_id,
            current.transaction_type,
            current.quantity,
            current.exchange,
            current.tradingsymbol,
            current.order_type,
            current.pending_quantity
        );
        let request = json!({
            "variety": current.variety,
            "order_id": current.order_id,
            "parent_order_id": args.parent_order_id.or(current.parent_order_id.clone()),
        });
        self.gate(
            "cancel_order",
            request,
            summary,
            json!({ "order": current }),
        )
        .await
    }

    async fn preview_convert(&self, mut a: ConvertPositionArgs) -> Result<Value> {
        a.exchange = a.exchange.trim().to_ascii_uppercase();
        a.tradingsymbol = a.tradingsymbol.trim().to_ascii_uppercase();
        a.transaction_type = one_of("transaction_type", &a.transaction_type, SIDES)?;
        a.old_product = one_of("old_product", &a.old_product, PRODUCTS)?;
        a.new_product = one_of("new_product", &a.new_product, PRODUCTS)?;
        a.position_type = a.position_type.trim().to_ascii_lowercase();
        if !matches!(a.position_type.as_str(), "overnight" | "day") {
            return Err(KiteError::InvalidInput(
                "position_type must be `overnight` or `day`".into(),
            ));
        }
        if a.quantity <= 0 {
            return Err(KiteError::InvalidInput("quantity must be positive".into()));
        }
        let c = &self.safety.config;
        if !c.allowed_products.is_empty()
            && !c
                .allowed_products
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&a.new_product))
        {
            return Err(KiteError::Blocked(format!(
                "product {} is not in allowed_products",
                a.new_product
            )));
        }
        let summary = format!(
            "CONVERT {} {} {}:{} ({}) from {} to {}",
            a.transaction_type,
            a.quantity,
            a.exchange,
            a.tradingsymbol,
            a.position_type,
            a.old_product,
            a.new_product
        );
        let request = serde_json::to_value(&a).map_err(|e| KiteError::Decode(e.to_string()))?;
        self.gate("convert_position", request.clone(), summary, request)
            .await
    }
}

/// Maps a modify-form key to a `'static` name, rejecting unknown keys.
fn static_key(k: &str) -> Option<&'static str> {
    [
        "order_type",
        "quantity",
        "price",
        "trigger_price",
        "disclosed_quantity",
        "validity",
        "parent_order_id",
        "market_protection",
    ]
    .into_iter()
    .find(|s| *s == k)
}

/// Converts a JSON object of string values into form pairs (keys must be known alert fields).
pub(crate) fn value_form(v: &Value) -> Form {
    const KEYS: &[&str] = &[
        "name",
        "type",
        "lhs_exchange",
        "lhs_tradingsymbol",
        "lhs_attribute",
        "operator",
        "rhs_type",
        "rhs_constant",
        "rhs_exchange",
        "rhs_tradingsymbol",
        "rhs_attribute",
        "basket",
    ];
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let key = KEYS.iter().find(|s| **s == k)?;
                    Some((*key, v.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tool_router(router = trading_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Place an order (equity, F&O, currency, commodity). Validates the instrument, lot
    /// size and safety limits, and previews margin + charges. With confirmation enabled
    /// (default) this returns a `confirm_token`; nothing is sent until `confirm_action`.
    #[tool(annotations(title = "Place order", destructive_hint = true, open_world_hint = true))]
    async fn place_order(&self, Parameters(args): Parameters<PlaceOrderArgs>) -> ToolResult {
        let result = self.preview_place(args).await;
        self.respond(result)
    }

    /// Modify an open order's price, quantity, trigger, type or validity.
    #[tool(annotations(
        title = "Modify order",
        destructive_hint = true,
        open_world_hint = true
    ))]
    async fn modify_order(&self, Parameters(args): Parameters<ModifyOrderArgs>) -> ToolResult {
        let result = self.preview_modify(args).await;
        self.respond(result)
    }

    /// Cancel an open order.
    #[tool(annotations(
        title = "Cancel order",
        destructive_hint = true,
        open_world_hint = true
    ))]
    async fn cancel_order(&self, Parameters(args): Parameters<CancelOrderArgs>) -> ToolResult {
        let result = self.preview_cancel(args).await;
        self.respond(result)
    }

    /// Convert an open position's product (e.g. intraday MIS -> delivery CNC, or MIS -> NRML).
    #[tool(annotations(
        title = "Convert position",
        destructive_hint = true,
        open_world_hint = true
    ))]
    async fn convert_position(
        &self,
        Parameters(args): Parameters<ConvertPositionArgs>,
    ) -> ToolResult {
        let result = self.preview_convert(args).await;
        self.respond(result)
    }

    /// Execute a previewed action (order, modification, cancellation, conversion, GTT,
    /// order-placing alert). Only call this after the user explicitly approved the preview.
    #[tool(annotations(
        title = "Confirm action",
        destructive_hint = true,
        open_world_hint = true
    ))]
    async fn confirm_action(&self, Parameters(args): Parameters<ConfirmArgs>) -> ToolResult {
        let result = match self.safety.confirmations.take(&args.confirm_token) {
            Some(p) => self.execute(&p.action, p.request).await,
            None => Err(KiteError::InvalidInput(
                "confirm_token is unknown, already used, or expired (2 minutes). Preview the action again.".into(),
            )),
        };
        self.respond(result)
    }

    /// Start CDSL/TPIN authorisation so holdings can be sold. Returns a URL the user opens
    /// to approve with their depository.
    #[tool(annotations(
        title = "Authorise holdings for sale",
        destructive_hint = false,
        open_world_hint = true
    ))]
    async fn authorise_holdings(&self, Parameters(args): Parameters<AuthoriseArgs>) -> ToolResult {
        let items: Vec<(String, i64)> = args
            .items
            .iter()
            .map(|i| (i.isin.trim().to_ascii_uppercase(), i.quantity))
            .collect();
        let result = self.client.authorise_holdings(&items).await.map(|v| {
            let request_id = v["request_id"].as_str().unwrap_or_default().to_string();
            let url = format!(
                "https://kite.zerodha.com/connect/portfolio/authorise/holdings/{}/{}",
                self.client.api_key(),
                request_id
            );
            json!({ "request_id": request_id, "authorise_url": url })
        });
        if let Ok(v) = &result {
            self.safety
                .audit
                .record("authorise_holdings", Outcome::Ok, &json!(items), v);
        }
        self.respond(result)
    }
}

#[tool_router(router = safety_router, vis = "pub(crate)")]
impl KiteMcp {
    /// List previewed actions still waiting for `confirm_action` (they expire after 2 minutes).
    #[tool(annotations(title = "Pending actions", read_only_hint = true))]
    async fn list_pending_actions(&self) -> ToolResult {
        let list: Vec<Value> = self
            .safety
            .confirmations
            .list()
            .into_iter()
            .map(|(token, a, left)| json!({ "confirm_token": token, "action": a.action, "summary": a.summary, "expires_in_seconds": left }))
            .collect();
        self.respond(Ok(list))
    }

    /// Show the trading safety settings: read-only, confirmation, dry-run and limits.
    #[tool(annotations(title = "Safety settings", read_only_hint = true))]
    async fn get_safety_settings(&self) -> ToolResult {
        let c = &self.safety.config;
        self.respond(Ok(json!({
            "read_only": c.read_only,
            "require_confirmation": c.require_confirmation,
            "dry_run": c.dry_run,
            "allowed_exchanges": c.allowed_exchanges,
            "allowed_products": c.allowed_products,
            "blocked_symbols": c.blocked_symbols,
            "order_tag": c.order_tag,
        })))
    }

    /// Show the most recent entries of the local audit log of mutating actions.
    #[tool(annotations(title = "Audit log", read_only_hint = true))]
    async fn get_audit_log(&self, Parameters(args): Parameters<AuditArgs>) -> ToolResult {
        self.respond(Ok(self.safety.audit.recent(args.limit.unwrap_or(20))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PlaceOrderArgs {
        serde_json::from_value(json!({
            "exchange": "nse", "tradingsymbol": "infy", "transaction_type": "buy",
            "order_type": "limit", "quantity": 1, "product": "cnc", "price": 1500.0
        }))
        .unwrap()
    }

    #[test]
    fn normalizes_and_builds_form() {
        let a = base().normalize().unwrap();
        assert_eq!(a.exchange, "NSE");
        assert_eq!(a.variety.as_deref(), Some("regular"));
        let form = a.form("kitemcp");
        assert!(form.contains(&("price", "1500".into())));
        assert!(form.contains(&("tag", "kitemcp".into())));
        assert!(form.contains(&("validity", "DAY".into())));
    }

    #[test]
    fn rejects_bad_combinations() {
        let mut a = base();
        a.price = None;
        assert!(a.normalize().is_err());

        let mut a = base();
        a.order_type = "SL".into();
        assert!(a.normalize().is_err()); // missing trigger

        let mut a = base();
        a.product = "XYZ".into();
        assert!(a.normalize().is_err());
    }

    #[test]
    fn market_orders_get_auto_protection() {
        let mut a = base();
        a.order_type = "MARKET".into();
        a.price = None;
        let a = a.normalize().unwrap();
        assert_eq!(a.market_protection, Some(-1.0));
    }
}
