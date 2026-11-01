use chrono::{FixedOffset, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{KiteError, Result};
use crate::instruments::search::{SearchQuery, search};
use crate::instruments::{history, normalize_quote_key};
use crate::kite::models::market::Instrument;
use crate::mcp::server::{KiteMcp, ToolResult};

const MAX_QUOTE_INSTRUMENTS: usize = 500;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Text matched against tradingsymbol and company/underlying name, e.g. "INFY",
    /// "reliance", "NIFTY", "BANKNIFTY26NOV".
    pub query: String,
    /// NSE, BSE, NFO, BFO, MCX, CDS, BCD.
    #[serde(default)]
    pub exchange: Option<String>,
    /// EQ, FUT, CE or PE.
    #[serde(default)]
    pub instrument_type: Option<String>,
    /// Exact expiry date YYYY-MM-DD (derivatives).
    #[serde(default)]
    pub expiry: Option<String>,
    /// Exact strike price (options).
    #[serde(default)]
    pub strike: Option<f64>,
    /// Max results (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct InstrumentRow<'a> {
    key: String,
    instrument_token: u64,
    tradingsymbol: &'a str,
    name: &'a str,
    exchange: &'a str,
    segment: &'a str,
    instrument_type: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    expiry: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    strike: f64,
    lot_size: i64,
    tick_size: f64,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

impl<'a> From<&'a Instrument> for InstrumentRow<'a> {
    fn from(i: &'a Instrument) -> Self {
        Self {
            key: format!("{}:{}", i.exchange, i.tradingsymbol),
            instrument_token: i.instrument_token,
            tradingsymbol: &i.tradingsymbol,
            name: &i.name,
            exchange: &i.exchange,
            segment: &i.segment,
            instrument_type: &i.instrument_type,
            expiry: &i.expiry,
            strike: i.strike,
            lot_size: i.lot_size,
            tick_size: i.tick_size,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InstrumentArgs {
    /// EXCHANGE:TRADINGSYMBOL (e.g. NSE:INFY), a bare NSE symbol, or an instrument token.
    pub instrument: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuoteArgs {
    /// Up to 500 instruments: EXCHANGE:TRADINGSYMBOL (e.g. NSE:INFY, NFO:NIFTY26NOVFUT,
    /// NSE:NIFTY 50) or instrument tokens. Bare symbols are treated as NSE.
    pub instruments: Vec<String>,
    /// Include 5-level market depth (bids/offers). Default false.
    #[serde(default)]
    pub include_depth: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LtpArgs {
    /// EXCHANGE:TRADINGSYMBOL keys or instrument tokens. Bare symbols are treated as NSE.
    pub instruments: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoricalArgs {
    /// EXCHANGE:TRADINGSYMBOL (e.g. NSE:INFY), a bare NSE symbol, or an instrument token.
    pub instrument: String,
    /// minute, 3minute, 5minute, 10minute, 15minute, 30minute, 60minute, or day.
    pub interval: String,
    /// Start, `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS` (IST).
    pub from: String,
    /// End, same formats (IST). Defaults to now.
    #[serde(default)]
    pub to: Option<String>,
    /// Continuous data across expiries (futures, `day` interval only).
    #[serde(default)]
    pub continuous: bool,
    /// Include open interest as a 7th column.
    #[serde(default)]
    pub oi: bool,
    /// Return at most this many candles, keeping the most recent (default 500).
    #[serde(default)]
    pub max_candles: Option<usize>,
}

fn validate_count(n: usize, max: usize) -> Result<()> {
    if n == 0 {
        return Err(KiteError::InvalidInput(
            "pass at least one instrument".into(),
        ));
    }
    if n > max {
        return Err(KiteError::InvalidInput(format!(
            "at most {max} instruments per call, got {n}"
        )));
    }
    Ok(())
}

fn now_ist_string() -> String {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset");
    Utc::now()
        .with_timezone(&ist)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

#[tool_router(router = market_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Search the instrument master (stocks, indices, futures, options, currency,
    /// commodities) by symbol or name. Use it to find exact tradingsymbols, tokens, lot
    /// sizes, expiries and strikes before quoting or trading derivatives.
    #[tool(annotations(
        title = "Search instruments",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn search_instruments(&self, Parameters(args): Parameters<SearchArgs>) -> ToolResult {
        let result = self.instruments.get().await.map(|idx| {
            let q = SearchQuery {
                text: args.query,
                exchange: args.exchange,
                instrument_type: args.instrument_type,
                segment: None,
                expiry: args.expiry,
                strike: args.strike,
                limit: args.limit.unwrap_or(20).min(200),
            };
            search(&idx, &q)
                .into_iter()
                .map(|i| serde_json::to_value(InstrumentRow::from(i)).unwrap_or_default())
                .collect::<Vec<_>>()
        });
        self.respond(result)
    }

    /// Look up one instrument's details (token, lot size, tick size, expiry, strike).
    #[tool(annotations(
        title = "Get instrument",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_instrument(&self, Parameters(args): Parameters<InstrumentArgs>) -> ToolResult {
        let result = match self.instruments.get().await {
            Ok(idx) => idx
                .resolve(&args.instrument)
                .map(|i| serde_json::to_value(InstrumentRow::from(i)).unwrap_or_default()),
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Get full market quotes: last price, OHLC, volume, average price, open interest,
    /// circuit limits, buy/sell quantity and optionally market depth.
    #[tool(annotations(title = "Get quote", read_only_hint = true, open_world_hint = true))]
    async fn get_quote(&self, Parameters(args): Parameters<QuoteArgs>) -> ToolResult {
        let keys: Vec<String> = args
            .instruments
            .iter()
            .map(|i| normalize_quote_key(i))
            .collect();
        let result = match validate_count(keys.len(), MAX_QUOTE_INSTRUMENTS) {
            Ok(()) => self.client.quote(&keys).await.map(|mut quotes| {
                if !args.include_depth {
                    quotes.values_mut().for_each(|q| q.depth = None);
                }
                quotes
            }),
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Get open/high/low/close and last price for instruments.
    #[tool(annotations(title = "Get OHLC", read_only_hint = true, open_world_hint = true))]
    async fn get_ohlc(&self, Parameters(args): Parameters<LtpArgs>) -> ToolResult {
        let keys: Vec<String> = args
            .instruments
            .iter()
            .map(|i| normalize_quote_key(i))
            .collect();
        let result = match validate_count(keys.len(), MAX_QUOTE_INSTRUMENTS) {
            Ok(()) => self.client.ohlc(&keys).await,
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Get the last traded price for up to 1000 instruments. Cheapest quote call.
    #[tool(annotations(title = "Get LTP", read_only_hint = true, open_world_hint = true))]
    async fn get_ltp(&self, Parameters(args): Parameters<LtpArgs>) -> ToolResult {
        let keys: Vec<String> = args
            .instruments
            .iter()
            .map(|i| normalize_quote_key(i))
            .collect();
        let result = match validate_count(keys.len(), 1000) {
            Ok(()) => self.client.ltp(&keys).await.map(|m| {
                m.into_iter()
                    .map(|(k, v)| (k, v.last_price))
                    .collect::<std::collections::BTreeMap<_, _>>()
            }),
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Get historical OHLCV candles. Long ranges are fetched in multiple requests
    /// automatically. Candles are `[time, open, high, low, close, volume(, oi)]`.
    #[tool(annotations(
        title = "Get historical candles",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_historical_data(
        &self,
        Parameters(args): Parameters<HistoricalArgs>,
    ) -> ToolResult {
        let result = self.historical(args).await;
        self.respond(result)
    }
}

impl KiteMcp {
    async fn historical(&self, args: HistoricalArgs) -> Result<serde_json::Value> {
        let interval = args.interval.trim().to_ascii_lowercase();
        let from = history::parse_datetime(&args.from, false)?;
        let to = match args.to.as_deref() {
            Some(t) => history::parse_datetime(t, true)?,
            None => history::parse_datetime(&now_ist_string(), true)?,
        };
        let windows = history::windows(&interval, from, to)?;
        if windows.len() > 40 {
            return Err(KiteError::InvalidInput(format!(
                "range needs {} requests at `{interval}`; narrow the dates or use a larger interval",
                windows.len()
            )));
        }

        let idx = self.instruments.get().await?;
        let inst = idx.resolve(&args.instrument)?;
        let mut candles = Vec::new();
        for (f, t) in &windows {
            let part = self
                .client
                .historical(
                    inst.instrument_token,
                    &interval,
                    f,
                    t,
                    args.continuous,
                    args.oi,
                )
                .await?;
            candles.extend(part.candles);
        }

        let total = candles.len();
        let max = args.max_candles.unwrap_or(500).max(1);
        let truncated = total > max;
        if truncated {
            candles.drain(..total - max);
        }
        let mut columns = vec!["time", "open", "high", "low", "close", "volume"];
        if args.oi {
            columns.push("oi");
        }
        Ok(serde_json::json!({
            "instrument": format!("{}:{}", inst.exchange, inst.tradingsymbol),
            "instrument_token": inst.instrument_token,
            "interval": interval,
            "from": windows.first().map(|w| &w.0),
            "to": windows.last().map(|w| &w.1),
            "total_candles": total,
            "returned": candles.len(),
            "truncated_to_latest": truncated,
            "columns": columns,
            "candles": candles,
        }))
    }
}
