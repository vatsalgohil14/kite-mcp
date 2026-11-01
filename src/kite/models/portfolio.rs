use serde::{Deserialize, Serialize};

/// `GET /portfolio/holdings` item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Holding {
    pub tradingsymbol: String,
    pub exchange: String,
    pub instrument_token: u64,
    pub isin: String,
    pub product: String,
    pub quantity: i64,
    pub t1_quantity: i64,
    pub realised_quantity: i64,
    pub used_quantity: i64,
    pub authorised_quantity: i64,
    pub opening_quantity: i64,
    pub collateral_quantity: i64,
    pub collateral_type: String,
    pub discrepancy: bool,
    pub average_price: f64,
    pub last_price: f64,
    pub close_price: f64,
    pub pnl: f64,
    pub day_change: f64,
    pub day_change_percentage: f64,
}

impl Holding {
    /// Settled plus T1 (bought, not yet delivered) quantity.
    pub fn total_quantity(&self) -> i64 {
        self.quantity + self.t1_quantity
    }
}

/// `GET /portfolio/positions` response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Positions {
    pub net: Vec<Position>,
    pub day: Vec<Position>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    pub tradingsymbol: String,
    pub exchange: String,
    pub instrument_token: u64,
    pub product: String,
    pub quantity: i64,
    pub overnight_quantity: i64,
    pub multiplier: f64,
    pub average_price: f64,
    pub close_price: f64,
    pub last_price: f64,
    pub value: f64,
    pub pnl: f64,
    pub m2m: f64,
    pub unrealised: f64,
    pub realised: f64,
    pub buy_quantity: i64,
    pub buy_price: f64,
    pub buy_value: f64,
    pub sell_quantity: i64,
    pub sell_price: f64,
    pub sell_value: f64,
    pub day_buy_quantity: i64,
    pub day_buy_price: f64,
    pub day_buy_value: f64,
    pub day_sell_quantity: i64,
    pub day_sell_price: f64,
    pub day_sell_value: f64,
}
