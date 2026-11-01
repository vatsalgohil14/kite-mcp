use serde::{Deserialize, Serialize};

/// `GET /orders` and `GET /orders/:id` item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Order {
    pub order_id: String,
    pub exchange_order_id: Option<String>,
    pub parent_order_id: Option<String>,
    pub placed_by: String,
    pub status: String,
    pub status_message: Option<String>,
    pub order_timestamp: Option<String>,
    pub exchange_timestamp: Option<String>,
    pub exchange_update_timestamp: Option<String>,
    pub variety: String,
    pub modified: bool,
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u64,
    pub order_type: String,
    pub transaction_type: String,
    pub validity: String,
    pub validity_ttl: Option<i64>,
    pub product: String,
    pub quantity: i64,
    pub disclosed_quantity: i64,
    pub price: f64,
    pub trigger_price: f64,
    pub average_price: f64,
    pub filled_quantity: i64,
    pub pending_quantity: i64,
    pub cancelled_quantity: i64,
    pub market_protection: f64,
    pub tag: Option<String>,
    pub tags: Option<Vec<String>>,
}

impl Order {
    /// Statuses that can still be modified or cancelled.
    pub fn is_open(&self) -> bool {
        !matches!(self.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED")
    }
}

/// `GET /trades` and `GET /orders/:id/trades` item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Trade {
    pub trade_id: String,
    pub order_id: String,
    pub exchange_order_id: Option<String>,
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u64,
    pub product: String,
    pub transaction_type: String,
    pub average_price: f64,
    pub quantity: i64,
    pub fill_timestamp: Option<String>,
    pub exchange_timestamp: Option<String>,
}
