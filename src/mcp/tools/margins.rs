use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::KiteError;
use crate::mcp::server::{KiteMcp, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MarginOrder {
    pub exchange: String,
    pub tradingsymbol: String,
    /// BUY or SELL.
    pub transaction_type: String,
    /// regular (default), amo, co, iceberg.
    #[serde(default)]
    pub variety: Option<String>,
    /// CNC, MIS, NRML, MTF.
    pub product: String,
    /// MARKET, LIMIT, SL, SL-M.
    pub order_type: String,
    pub quantity: i64,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub trigger_price: Option<f64>,
}

impl MarginOrder {
    fn to_json(&self) -> Value {
        json!({
            "exchange": self.exchange.trim().to_ascii_uppercase(),
            "tradingsymbol": self.tradingsymbol.trim().to_ascii_uppercase(),
            "transaction_type": self.transaction_type.trim().to_ascii_uppercase(),
            "variety": self.variety.as_deref().unwrap_or("regular"),
            "product": self.product.trim().to_ascii_uppercase(),
            "order_type": self.order_type.trim().to_ascii_uppercase(),
            "quantity": self.quantity,
            "price": self.price.unwrap_or(0.0),
            "trigger_price": self.trigger_price.unwrap_or(0.0),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OrderMarginsArgs {
    pub orders: Vec<MarginOrder>,
    /// Only return totals (default false).
    #[serde(default)]
    pub compact: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BasketMarginsArgs {
    pub orders: Vec<MarginOrder>,
    /// Account for existing positions (default true).
    #[serde(default = "yes")]
    pub consider_positions: bool,
    #[serde(default)]
    pub compact: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChargeOrder {
    pub exchange: String,
    pub tradingsymbol: String,
    /// BUY or SELL.
    pub transaction_type: String,
    #[serde(default)]
    pub variety: Option<String>,
    pub product: String,
    pub order_type: String,
    pub quantity: i64,
    /// Execution price (must be non-zero).
    pub average_price: f64,
    /// Any identifier; defaults to a sequence number.
    #[serde(default)]
    pub order_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChargesArgs {
    pub orders: Vec<ChargeOrder>,
}

fn non_empty<T>(v: &[T]) -> Result<(), KiteError> {
    if v.is_empty() {
        Err(KiteError::InvalidInput("pass at least one order".into()))
    } else {
        Ok(())
    }
}

#[tool_router(router = margins_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Calculate the margin required for each order (SPAN, exposure, option premium,
    /// leverage) plus a breakdown of charges, considering existing positions.
    #[tool(annotations(title = "Order margins", read_only_hint = true, open_world_hint = true))]
    async fn get_order_margins(
        &self,
        Parameters(args): Parameters<OrderMarginsArgs>,
    ) -> ToolResult {
        let body = Value::Array(args.orders.iter().map(MarginOrder::to_json).collect());
        let result = match non_empty(&args.orders) {
            Ok(()) => self.client.order_margins(&body, args.compact).await,
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Calculate the combined margin for a basket (e.g. an option spread), including the
    /// spread/hedge benefit (`initial` vs `final`).
    #[tool(annotations(
        title = "Basket margins",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_basket_margins(
        &self,
        Parameters(args): Parameters<BasketMarginsArgs>,
    ) -> ToolResult {
        let body = Value::Array(args.orders.iter().map(MarginOrder::to_json).collect());
        let result = match non_empty(&args.orders) {
            Ok(()) => {
                self.client
                    .basket_margins(&body, args.consider_positions, args.compact)
                    .await
            }
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Calculate brokerage, STT/CTT, exchange and SEBI charges, stamp duty and GST for
    /// executed (or hypothetical) orders — a virtual contract note.
    #[tool(annotations(title = "Order charges", read_only_hint = true, open_world_hint = true))]
    async fn get_order_charges(&self, Parameters(args): Parameters<ChargesArgs>) -> ToolResult {
        let body = Value::Array(
            args.orders
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    json!({
                        "order_id": o.order_id.clone().unwrap_or_else(|| (i + 1).to_string()),
                        "exchange": o.exchange.trim().to_ascii_uppercase(),
                        "tradingsymbol": o.tradingsymbol.trim().to_ascii_uppercase(),
                        "transaction_type": o.transaction_type.trim().to_ascii_uppercase(),
                        "variety": o.variety.as_deref().unwrap_or("regular"),
                        "product": o.product.trim().to_ascii_uppercase(),
                        "order_type": o.order_type.trim().to_ascii_uppercase(),
                        "quantity": o.quantity,
                        "average_price": o.average_price,
                    })
                })
                .collect(),
        );
        let result = match non_empty(&args.orders) {
            Ok(()) => self.client.order_charges(&body).await,
            Err(e) => Err(e),
        };
        self.respond(result)
    }
}
