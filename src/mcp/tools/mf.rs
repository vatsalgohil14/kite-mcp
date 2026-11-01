//! Mutual funds: orders, SIPs, holdings and the fund master (read-only).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::{pct, r2};
use crate::error::{KiteError, Result};
use crate::instruments::last_refresh_boundary;
use crate::kite::KiteClient;
use crate::mcp::server::{KiteMcp, ToolResult};

type FundRow = HashMap<String, String>;
/// Fetch time and parsed rows.
type FundList = (chrono::DateTime<Utc>, Arc<Vec<FundRow>>);

/// In-memory cache of the MF instrument CSV, refreshed daily.
#[derive(Default)]
pub struct MfCache {
    inner: Mutex<Option<FundList>>,
}

impl MfCache {
    pub async fn get(&self, client: &KiteClient) -> Result<Arc<Vec<FundRow>>> {
        let mut guard = self.inner.lock().await;
        if let Some((at, rows)) = guard.as_ref()
            && *at >= last_refresh_boundary(Utc::now())
        {
            return Ok(rows.clone());
        }
        let text = client.mf_instruments_csv().await?;
        let mut reader = csv::Reader::from_reader(text.as_bytes());
        let rows: Vec<FundRow> = reader.deserialize().filter_map(|r| r.ok()).collect();
        if rows.is_empty() {
            return Err(KiteError::Decode("MF instrument list is empty".into()));
        }
        let rows = Arc::new(rows);
        *guard = Some((Utc::now(), rows.clone()));
        Ok(rows)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MfOrderIdArgs {
    pub order_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MfSearchArgs {
    /// Matched against fund name, AMC and ISIN (case-insensitive), e.g. "parag parikh flexi".
    pub query: String,
    /// `direct` or `regular`.
    #[serde(default)]
    pub plan: Option<String>,
    /// `growth` or `payout`.
    #[serde(default)]
    pub dividend_type: Option<String>,
    /// e.g. equity, debt, elss, hybrid, liquid.
    #[serde(default)]
    pub scheme_type: Option<String>,
    /// Max results (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

fn field_eq(row: &FundRow, key: &str, want: &Option<String>) -> bool {
    want.as_ref().is_none_or(|w| {
        row.get(key)
            .is_some_and(|v| v.eq_ignore_ascii_case(w.trim()))
    })
}

#[tool_router(router = mf_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Mutual fund orders from the last 7 days.
    #[tool(annotations(title = "Get MF orders", read_only_hint = true, open_world_hint = true))]
    async fn get_mf_orders(&self) -> ToolResult {
        let result = self.client.mf_orders().await;
        self.respond(result)
    }

    /// One mutual fund order.
    #[tool(annotations(title = "Get MF order", read_only_hint = true, open_world_hint = true))]
    async fn get_mf_order(&self, Parameters(args): Parameters<MfOrderIdArgs>) -> ToolResult {
        let result = self.client.mf_order(args.order_id.trim()).await;
        self.respond(result)
    }

    /// Active and paused SIPs with amount, frequency, next instalment and progress.
    #[tool(annotations(title = "Get SIPs", read_only_hint = true, open_world_hint = true))]
    async fn get_mf_sips(&self) -> ToolResult {
        let result = self.client.mf_sips().await;
        self.respond(result)
    }

    /// Mutual fund holdings with invested amount, current value and returns.
    #[tool(annotations(
        title = "Get MF holdings",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_mf_holdings(&self) -> ToolResult {
        let result = self.client.mf_holdings().await.map(|list| {
            let rows: Vec<serde_json::Value> = list
                .iter()
                .map(|h| {
                    let qty = h["quantity"].as_f64().unwrap_or(0.0);
                    let avg = h["average_price"].as_f64().unwrap_or(0.0);
                    let ltp = h["last_price"].as_f64().unwrap_or(0.0);
                    let invested = qty * avg;
                    let current = qty * ltp;
                    serde_json::json!({
                        "fund": h["fund"], "isin": h["tradingsymbol"], "folio": h["folio"],
                        "units": qty, "avg_nav": avg, "nav": ltp, "nav_date": h["last_price_date"],
                        "invested": r2(invested), "current_value": r2(current),
                        "pnl": r2(current - invested), "pnl_pct": pct(current - invested, invested),
                    })
                })
                .collect();
            let invested: f64 = rows
                .iter()
                .map(|r| r["invested"].as_f64().unwrap_or(0.0))
                .sum();
            let current: f64 = rows
                .iter()
                .map(|r| r["current_value"].as_f64().unwrap_or(0.0))
                .sum();
            serde_json::json!({
                "summary": {
                    "count": rows.len(), "invested": r2(invested), "current_value": r2(current),
                    "pnl": r2(current - invested), "pnl_pct": pct(current - invested, invested),
                },
                "holdings": rows,
            })
        });
        self.respond(result)
    }

    /// Search the mutual fund master by name, AMC or ISIN. Returns NAV, plan, scheme type
    /// and minimum purchase amounts.
    #[tool(annotations(
        title = "Search mutual funds",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn search_mf_instruments(
        &self,
        Parameters(args): Parameters<MfSearchArgs>,
    ) -> ToolResult {
        let result = self.mf_cache.get(&self.client).await.map(|rows| {
            let words: Vec<String> = args
                .query
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect();
            rows.iter()
                .filter(|r| field_eq(r, "plan", &args.plan))
                .filter(|r| field_eq(r, "dividend_type", &args.dividend_type))
                .filter(|r| field_eq(r, "scheme_type", &args.scheme_type))
                .filter(|r| {
                    let hay = format!(
                        "{} {} {}",
                        r.get("name").map(String::as_str).unwrap_or_default(),
                        r.get("amc").map(String::as_str).unwrap_or_default(),
                        r.get("tradingsymbol")
                            .map(String::as_str)
                            .unwrap_or_default()
                    )
                    .to_ascii_lowercase();
                    words.iter().all(|w| hay.contains(w))
                })
                .take(args.limit.unwrap_or(20).min(100))
                .cloned()
                .collect::<Vec<_>>()
        });
        self.respond(result)
    }
}
