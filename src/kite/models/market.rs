use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ohlc {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DepthLevel {
    pub price: f64,
    pub quantity: i64,
    pub orders: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Depth {
    pub buy: Vec<DepthLevel>,
    pub sell: Vec<DepthLevel>,
}

/// `GET /quote` item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Quote {
    pub instrument_token: u64,
    pub timestamp: Option<String>,
    pub last_trade_time: Option<String>,
    pub last_price: f64,
    pub last_quantity: i64,
    pub buy_quantity: i64,
    pub sell_quantity: i64,
    pub volume: i64,
    pub average_price: f64,
    pub oi: f64,
    pub oi_day_high: f64,
    pub oi_day_low: f64,
    pub net_change: f64,
    pub lower_circuit_limit: f64,
    pub upper_circuit_limit: f64,
    pub ohlc: Ohlc,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<Depth>,
}

/// `GET /quote/ohlc` item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OhlcQuote {
    pub instrument_token: u64,
    pub last_price: f64,
    pub ohlc: Ohlc,
}

/// `GET /quote/ltp` item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LtpQuote {
    pub instrument_token: u64,
    pub last_price: f64,
}

pub type QuoteMap<T> = BTreeMap<String, T>;

/// `GET /instruments/historical` response. Each candle is
/// `[timestamp, open, high, low, close, volume]` plus `oi` when requested.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoricalData {
    pub candles: Vec<Vec<serde_json::Value>>,
}

/// One row of the `/instruments` CSV dump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrument {
    pub instrument_token: u64,
    pub exchange_token: u64,
    pub tradingsymbol: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub last_price: f64,
    /// `YYYY-MM-DD`, empty for non-derivatives.
    #[serde(default)]
    pub expiry: String,
    #[serde(default)]
    pub strike: f64,
    #[serde(default)]
    pub tick_size: f64,
    #[serde(default)]
    pub lot_size: i64,
    pub instrument_type: String,
    pub segment: String,
    pub exchange: String,
}
