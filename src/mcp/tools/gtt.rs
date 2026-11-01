//! GTT (Good Till Triggered): orders that fire when a price trigger is hit. Single
//! trigger, or two-leg OCO (one cancels other) for target + stop-loss.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{KiteError, Result};
use crate::mcp::server::{KiteMcp, ToolResult};
use crate::safety::OrderCheck;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GttLeg {
    /// BUY or SELL.
    pub transaction_type: String,
    pub quantity: i64,
    /// LIMIT price placed when this leg triggers.
    pub price: f64,
    /// CNC, MIS, NRML.
    pub product: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GttArgs {
    /// `single` (one trigger, one order) or `two-leg` (OCO: lower trigger -> first order,
    /// upper trigger -> second order).
    #[serde(rename = "type")]
    pub kind: String,
    pub exchange: String,
    pub tradingsymbol: String,
    /// One value for single, two ascending values for two-leg.
    pub trigger_values: Vec<f64>,
    /// One leg for single, two for two-leg (in the same order as trigger_values).
    pub orders: Vec<GttLeg>,
    /// Current price of the instrument. Fetched automatically when market data is
    /// available; otherwise required.
    #[serde(default)]
    pub last_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModifyGttArgs {
    pub trigger_id: u64,
    #[serde(flatten)]
    pub gtt: GttArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GttIdArgs {
    pub trigger_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GttListArgs {
    /// Only GTTs with this status: active, triggered, disabled, expired, cancelled,
    /// rejected, deleted.
    #[serde(default)]
    pub status: Option<String>,
}

impl KiteMcp {
    /// Validates a GTT and returns the (summary, request) pair Kite expects.
    async fn build_gtt(&self, mut a: GttArgs) -> Result<(String, Value)> {
        a.kind = a.kind.trim().to_ascii_lowercase();
        a.exchange = a.exchange.trim().to_ascii_uppercase();
        a.tradingsymbol = a.tradingsymbol.trim().to_ascii_uppercase();
        let n = match a.kind.as_str() {
            "single" => 1,
            "two-leg" => 2,
            other => {
                return Err(KiteError::InvalidInput(format!(
                    "type must be `single` or `two-leg`, got `{other}`"
                )));
            }
        };
        if a.trigger_values.len() != n || a.orders.len() != n {
            return Err(KiteError::InvalidInput(format!(
                "{} GTT needs exactly {n} trigger value(s) and {n} order(s)",
                a.kind
            )));
        }
        if n == 2 && a.trigger_values[0] >= a.trigger_values[1] {
            return Err(KiteError::InvalidInput(
                "two-leg trigger_values must be ascending: [stop-loss trigger, target trigger]"
                    .into(),
            ));
        }
        let idx = self.instruments.get().await?;
        let inst = idx
            .by_symbol(&a.exchange, &a.tradingsymbol)
            .ok_or_else(|| KiteError::NotFound(format!("{}:{}", a.exchange, a.tradingsymbol)))?;
        let last_price = match a.last_price {
            Some(p) => p,
            None => self
                .try_ltp(&a.exchange, &a.tradingsymbol)
                .await
                .ok_or_else(|| {
                    KiteError::InvalidInput(
                        "pass `last_price` (live price unavailable on this plan)".into(),
                    )
                })?,
        };
        let mut legs = Vec::new();
        for leg in &a.orders {
            let side = leg.transaction_type.trim().to_ascii_uppercase();
            let product = leg.product.trim().to_ascii_uppercase();
            if inst.lot_size > 1 && leg.quantity % inst.lot_size != 0 {
                return Err(KiteError::InvalidInput(format!(
                    "quantity {} is not a multiple of the lot size {}",
                    leg.quantity, inst.lot_size
                )));
            }
            self.safety.check_order(&OrderCheck {
                exchange: &a.exchange,
                tradingsymbol: &a.tradingsymbol,
                product: &product,
                quantity: leg.quantity,
                reference_price: Some(leg.price),
            })?;
            legs.push(json!({
                "exchange": a.exchange, "tradingsymbol": a.tradingsymbol,
                "transaction_type": side, "quantity": leg.quantity,
                "order_type": "LIMIT", "product": product, "price": leg.price,
            }));
        }
        let condition = json!({
            "exchange": a.exchange, "tradingsymbol": a.tradingsymbol,
            "trigger_values": a.trigger_values, "last_price": last_price,
        });
        let summary = format!(
            "GTT {} on {}:{} (LTP ₹{last_price}): {}",
            a.kind,
            a.exchange,
            a.tradingsymbol,
            a.trigger_values
                .iter()
                .zip(&a.orders)
                .map(|(t, o)| format!(
                    "at ₹{t} {} {} @ ₹{} {}",
                    o.transaction_type.to_ascii_uppercase(),
                    o.quantity,
                    o.price,
                    o.product.to_ascii_uppercase()
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        Ok((
            summary,
            json!({ "type": a.kind, "condition": condition, "orders": legs }),
        ))
    }
}

#[tool_router(router = gtt_read_router, vis = "pub(crate)")]
impl KiteMcp {
    /// List GTT triggers (active and recent), optionally filtered by status.
    #[tool(annotations(title = "Get GTTs", read_only_hint = true, open_world_hint = true))]
    async fn get_gtts(&self, Parameters(args): Parameters<GttListArgs>) -> ToolResult {
        let result = self.client.gtts().await.map(|mut list| {
            if let Some(s) = &args.status {
                list.retain(|g| {
                    g["status"]
                        .as_str()
                        .is_some_and(|x| x.eq_ignore_ascii_case(s))
                });
            }
            list
        });
        self.respond(result)
    }

    /// Get one GTT trigger with its condition, orders and status.
    #[tool(annotations(title = "Get GTT", read_only_hint = true, open_world_hint = true))]
    async fn get_gtt(&self, Parameters(args): Parameters<GttIdArgs>) -> ToolResult {
        let result = self.client.gtt(args.trigger_id).await;
        self.respond(result)
    }
}

#[tool_router(router = gtt_write_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Create a GTT: a LIMIT order placed automatically when the price hits a trigger.
    /// Use `two-leg` for an OCO target + stop-loss on an existing holding. Goes through
    /// the safety limits and confirmation.
    #[tool(annotations(title = "Place GTT", destructive_hint = true, open_world_hint = true))]
    async fn place_gtt(&self, Parameters(args): Parameters<GttArgs>) -> ToolResult {
        let result = match self.build_gtt(args).await {
            Ok((summary, request)) => {
                let preview = request.clone();
                self.gate("place_gtt", request, summary, preview).await
            }
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Replace an active GTT's triggers and orders.
    #[tool(annotations(title = "Modify GTT", destructive_hint = true, open_world_hint = true))]
    async fn modify_gtt(&self, Parameters(args): Parameters<ModifyGttArgs>) -> ToolResult {
        let id = args.trigger_id;
        let result = match self.build_gtt(args.gtt).await {
            Ok((summary, mut request)) => {
                request["trigger_id"] = id.into();
                let preview = request.clone();
                self.gate(
                    "modify_gtt",
                    request,
                    format!("MODIFY GTT {id}: {summary}"),
                    preview,
                )
                .await
            }
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Delete an active GTT.
    #[tool(annotations(title = "Delete GTT", destructive_hint = true, open_world_hint = true))]
    async fn delete_gtt(&self, Parameters(args): Parameters<GttIdArgs>) -> ToolResult {
        let id = args.trigger_id;
        let result = match self.client.gtt(id).await {
            Ok(current) => {
                let summary = format!(
                    "DELETE GTT {id} ({} on {}:{})",
                    current["type"].as_str().unwrap_or("?"),
                    current["condition"]["exchange"].as_str().unwrap_or("?"),
                    current["condition"]["tradingsymbol"]
                        .as_str()
                        .unwrap_or("?"),
                );
                self.gate("delete_gtt", json!({ "trigger_id": id }), summary, current)
                    .await
            }
            Err(e) => Err(e),
        };
        self.respond(result)
    }
}
