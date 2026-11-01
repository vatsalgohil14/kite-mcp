//! Composite tools: portfolio overview, market hours and option chains.

use chrono::{Datelike, FixedOffset, NaiveTime, Utc, Weekday};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{pct, r2};
use crate::error::{KiteError, Result};
use crate::instruments::search::{SearchQuery, search};
use crate::mcp::server::{KiteMcp, ToolResult};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OptionChainArgs {
    /// Underlying name as in the instrument list, e.g. NIFTY, BANKNIFTY, RELIANCE.
    pub underlying: String,
    /// Expiry YYYY-MM-DD; defaults to the nearest one.
    #[serde(default)]
    pub expiry: Option<String>,
    /// Strikes on each side of the at-the-money strike (default 10).
    #[serde(default)]
    pub strikes_around: Option<usize>,
    /// NFO (default) or BFO (e.g. SENSEX options).
    #[serde(default)]
    pub exchange: Option<String>,
}

/// Spot instrument for common index underlyings; stocks map to NSE:<name>.
fn spot_key(underlying: &str) -> String {
    match underlying {
        "NIFTY" => "NSE:NIFTY 50".into(),
        "BANKNIFTY" => "NSE:NIFTY BANK".into(),
        "FINNIFTY" => "NSE:NIFTY FIN SERVICE".into(),
        "MIDCPNIFTY" => "NSE:NIFTY MID SELECT".into(),
        "NIFTYNXT50" => "NSE:NIFTY NEXT 50".into(),
        "SENSEX" => "BSE:SENSEX".into(),
        "BANKEX" => "BSE:BANKEX".into(),
        other => format!("NSE:{other}"),
    }
}

/// Session state for a segment at a given IST time (weekends closed; exchange holidays
/// are not known here).
fn session(
    now_ist: chrono::DateTime<FixedOffset>,
    open: (u32, u32),
    close: (u32, u32),
) -> &'static str {
    if matches!(now_ist.weekday(), Weekday::Sat | Weekday::Sun) {
        return "closed (weekend)";
    }
    let t = now_ist.time();
    let o = NaiveTime::from_hms_opt(open.0, open.1, 0).expect("valid");
    let c = NaiveTime::from_hms_opt(close.0, close.1, 0).expect("valid");
    if t < o {
        "pre-market"
    } else if t < c {
        "open"
    } else {
        "closed"
    }
}

#[tool_router(router = insights_router, vis = "pub(crate)")]
impl KiteMcp {
    /// One-call overview: equity holdings value and P&L, open positions P&L, available
    /// funds, and mutual fund holdings.
    #[tool(annotations(
        title = "Portfolio summary",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn portfolio_summary(&self) -> ToolResult {
        let result: Result<Value> = async {
            let (holdings, positions, margins, mf) = tokio::join!(
                self.client.holdings(),
                self.client.positions(),
                self.client.margins(),
                self.client.mf_holdings(),
            );
            let holdings = holdings?;
            let invested: f64 = holdings.iter().map(|h| h.average_price * h.total_quantity() as f64).sum();
            let current: f64 = holdings.iter().map(|h| h.last_price * h.total_quantity() as f64).sum();
            let day: f64 = holdings.iter().map(|h| h.day_change * h.total_quantity() as f64).sum();
            let mut best = holdings.clone();
            best.sort_by(|a, b| b.day_change_percentage.total_cmp(&a.day_change_percentage));
            let movers = |list: &[crate::kite::models::portfolio::Holding]| {
                list.iter().take(3).map(|h| json!({ "symbol": h.tradingsymbol, "day_change_pct": r2(h.day_change_percentage) })).collect::<Vec<_>>()
            };
            let worst: Vec<_> = best.iter().rev().cloned().collect();

            let positions = positions.map(|p| {
                json!({
                    "open": p.net.iter().filter(|x| x.quantity != 0).count(),
                    "pnl": r2(p.net.iter().map(|x| x.pnl).sum()),
                    "m2m": r2(p.net.iter().map(|x| x.m2m).sum()),
                })
            });
            let funds = margins.map(|m| {
                json!({
                    "equity_available": m.equity.as_ref().map(|e| r2(e.net)),
                    "commodity_available": m.commodity.as_ref().map(|c| r2(c.net)),
                })
            });
            let mf = mf.map(|list| {
                let inv: f64 = list.iter().map(|h| h["quantity"].as_f64().unwrap_or(0.0) * h["average_price"].as_f64().unwrap_or(0.0)).sum();
                let cur: f64 = list.iter().map(|h| h["quantity"].as_f64().unwrap_or(0.0) * h["last_price"].as_f64().unwrap_or(0.0)).sum();
                json!({ "count": list.len(), "invested": r2(inv), "current_value": r2(cur), "pnl": r2(cur - inv), "pnl_pct": pct(cur - inv, inv) })
            });
            let err = |e: KiteError| json!({ "unavailable": e.to_string() });
            Ok(json!({
                "equity_holdings": {
                    "count": holdings.len(),
                    "invested": r2(invested),
                    "current_value": r2(current),
                    "pnl": r2(current - invested),
                    "pnl_pct": pct(current - invested, invested),
                    "day_change": r2(day),
                    "day_change_pct": pct(day, current - day),
                    "top_gainers_today": movers(&best),
                    "top_losers_today": movers(&worst),
                },
                "positions": positions.unwrap_or_else(err),
                "funds": funds.unwrap_or_else(err),
                "mutual_funds": mf.unwrap_or_else(err),
            }))
        }
        .await;
        self.respond(result)
    }

    /// Current IST time and whether equity, F&O, currency and commodity sessions are open
    /// (by regular hours; exchange holidays are not accounted for).
    #[tool(annotations(title = "Market status", read_only_hint = true))]
    async fn market_status(&self) -> ToolResult {
        let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset");
        let now = Utc::now().with_timezone(&ist);
        self.respond(Ok(json!({
            "now_ist": now.format("%Y-%m-%d %H:%M:%S %a").to_string(),
            "equity_and_fno": { "hours": "09:15-15:30 (pre-open 09:00-09:08)", "status": session(now, (9, 15), (15, 30)) },
            "currency": { "hours": "09:00-17:00", "status": session(now, (9, 0), (17, 0)) },
            "commodity_mcx": { "hours": "09:00-23:30 (varies by contract)", "status": session(now, (9, 0), (23, 30)) },
            "note": "Exchange holidays are not included; check the NSE holiday calendar.",
        })))
    }

    /// Option chain for an underlying and expiry: strikes around the money with CE/PE
    /// last price, OI, volume and bid/ask. Needs the paid market-data plan.
    #[tool(annotations(title = "Option chain", read_only_hint = true, open_world_hint = true))]
    async fn get_option_chain(&self, Parameters(args): Parameters<OptionChainArgs>) -> ToolResult {
        let result = self.option_chain(args).await;
        self.respond(result)
    }
}

impl KiteMcp {
    async fn option_chain(&self, args: OptionChainArgs) -> Result<Value> {
        let underlying = args.underlying.trim().to_ascii_uppercase();
        let exchange = args
            .exchange
            .as_deref()
            .unwrap_or("NFO")
            .trim()
            .to_ascii_uppercase();
        let idx = self.instruments.get().await?;
        let mut opts: Vec<_> = search(
            &idx,
            &SearchQuery {
                text: String::new(),
                exchange: Some(exchange.clone()),
                limit: usize::MAX,
                ..Default::default()
            },
        )
        .into_iter()
        .filter(|i| i.name == underlying && matches!(i.instrument_type.as_str(), "CE" | "PE"))
        .collect();
        if opts.is_empty() {
            return Err(KiteError::NotFound(format!(
                "no {exchange} options for `{underlying}`"
            )));
        }
        let expiry = match args.expiry {
            Some(e) => e.trim().to_string(),
            None => opts
                .iter()
                .map(|i| i.expiry.clone())
                .min()
                .unwrap_or_default(),
        };
        opts.retain(|i| i.expiry == expiry);
        if opts.is_empty() {
            return Err(KiteError::NotFound(format!(
                "no {underlying} options expiring {expiry}"
            )));
        }
        let mut strikes: Vec<f64> = opts.iter().map(|i| i.strike).collect();
        strikes.sort_by(f64::total_cmp);
        strikes.dedup();

        let spot_key = spot_key(&underlying);
        let spot = self
            .client
            .ltp(std::slice::from_ref(&spot_key))
            .await?
            .get(&spot_key)
            .map(|q| q.last_price);
        let atm_i = match spot {
            Some(s) => strikes
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - s).abs().total_cmp(&(b.1 - s).abs()))
                .map(|(i, _)| i)
                .unwrap_or(0),
            None => strikes.len() / 2,
        };
        let n = args.strikes_around.unwrap_or(10).min(50);
        let lo = atm_i.saturating_sub(n);
        let hi = (atm_i + n + 1).min(strikes.len());
        let window = &strikes[lo..hi];

        let keys: Vec<String> = opts
            .iter()
            .filter(|i| window.iter().any(|s| (s - i.strike).abs() < 1e-6))
            .map(|i| format!("{}:{}", i.exchange, i.tradingsymbol))
            .collect();
        let quotes = self.client.quote(&keys).await?;
        let leg = |strike: f64, kind: &str| -> Value {
            let Some(inst) = opts
                .iter()
                .find(|i| (i.strike - strike).abs() < 1e-6 && i.instrument_type == kind)
            else {
                return Value::Null;
            };
            let key = format!("{}:{}", inst.exchange, inst.tradingsymbol);
            let Some(q) = quotes.get(&key) else {
                return json!({ "symbol": inst.tradingsymbol });
            };
            let bid = q
                .depth
                .as_ref()
                .and_then(|d| d.buy.first())
                .map(|l| l.price);
            let ask = q
                .depth
                .as_ref()
                .and_then(|d| d.sell.first())
                .map(|l| l.price);
            json!({
                "symbol": inst.tradingsymbol, "ltp": q.last_price, "change": r2(q.net_change),
                "oi": q.oi, "volume": q.volume, "bid": bid, "ask": ask,
            })
        };
        let rows: Vec<Value> = window
            .iter()
            .map(|s| json!({ "strike": s, "CE": leg(*s, "CE"), "PE": leg(*s, "PE") }))
            .collect();
        let lot = opts.first().map(|i| i.lot_size).unwrap_or_default();
        Ok(json!({
            "underlying": underlying, "spot": spot, "spot_instrument": spot_key,
            "expiry": expiry, "lot_size": lot, "atm_strike": strikes.get(atm_i),
            "chain": rows,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_states() {
        let ist = FixedOffset::east_opt(19800).unwrap();
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&ist)
        };
        // 2026-11-02 is a Monday.
        assert_eq!(
            session(at("2026-11-02T09:00:00+05:30"), (9, 15), (15, 30)),
            "pre-market"
        );
        assert_eq!(
            session(at("2026-11-02T10:00:00+05:30"), (9, 15), (15, 30)),
            "open"
        );
        assert_eq!(
            session(at("2026-11-02T16:00:00+05:30"), (9, 15), (15, 30)),
            "closed"
        );
        assert_eq!(
            session(at("2026-11-01T10:00:00+05:30"), (9, 15), (15, 30)),
            "closed (weekend)"
        );
    }

    #[test]
    fn spot_keys() {
        assert_eq!(spot_key("NIFTY"), "NSE:NIFTY 50");
        assert_eq!(spot_key("RELIANCE"), "NSE:RELIANCE");
    }
}
