use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{pct, r2};
use crate::kite::models::portfolio::{Holding, Position};
use crate::mcp::server::{KiteMcp, ToolResult};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HoldingsArgs {
    /// Only holdings whose tradingsymbol contains this text (case-insensitive).
    #[serde(default)]
    pub symbol: Option<String>,
    /// Sort order: `value` (default), `pnl`, `pnl_pct`, `day_change_pct`, or `symbol`.
    #[serde(default)]
    pub sort_by: Option<String>,
    /// Max rows to return (default: all). The summary always covers all holdings.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct HoldingRow {
    symbol: String,
    exchange: String,
    quantity: i64,
    #[serde(skip_serializing_if = "is_zero")]
    t1_quantity: i64,
    avg_price: f64,
    ltp: f64,
    invested: f64,
    current_value: f64,
    pnl: f64,
    pnl_pct: f64,
    day_change: f64,
    day_change_pct: f64,
}

fn is_zero(x: &i64) -> bool {
    *x == 0
}

impl From<&Holding> for HoldingRow {
    fn from(h: &Holding) -> Self {
        let qty = h.total_quantity() as f64;
        let invested = h.average_price * qty;
        let current = h.last_price * qty;
        Self {
            symbol: h.tradingsymbol.clone(),
            exchange: h.exchange.clone(),
            quantity: h.quantity,
            t1_quantity: h.t1_quantity,
            avg_price: r2(h.average_price),
            ltp: r2(h.last_price),
            invested: r2(invested),
            current_value: r2(current),
            pnl: r2(current - invested),
            pnl_pct: pct(current - invested, invested),
            day_change: r2(h.day_change * qty),
            day_change_pct: r2(h.day_change_percentage),
        }
    }
}

#[derive(Debug, Serialize)]
struct HoldingsSummary {
    count: usize,
    invested: f64,
    current_value: f64,
    total_pnl: f64,
    total_pnl_pct: f64,
    day_change: f64,
    day_change_pct: f64,
}

fn summarize(rows: &[HoldingRow]) -> HoldingsSummary {
    let invested: f64 = rows.iter().map(|r| r.invested).sum();
    let current: f64 = rows.iter().map(|r| r.current_value).sum();
    let day: f64 = rows.iter().map(|r| r.day_change).sum();
    HoldingsSummary {
        count: rows.len(),
        invested: r2(invested),
        current_value: r2(current),
        total_pnl: r2(current - invested),
        total_pnl_pct: pct(current - invested, invested),
        day_change: r2(day),
        day_change_pct: pct(day, current - day),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PositionsArgs {
    /// `net` (default): positions carried and taken today. `day`: only today's activity.
    #[serde(default)]
    pub kind: Option<String>,
    /// Include squared-off positions (quantity 0). Default false.
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Debug, Serialize)]
struct PositionRow {
    symbol: String,
    exchange: String,
    product: String,
    quantity: i64,
    avg_price: f64,
    ltp: f64,
    pnl: f64,
    m2m: f64,
    realised: f64,
    unrealised: f64,
    buy_qty: i64,
    sell_qty: i64,
}

impl From<&Position> for PositionRow {
    fn from(p: &Position) -> Self {
        Self {
            symbol: p.tradingsymbol.clone(),
            exchange: p.exchange.clone(),
            product: p.product.clone(),
            quantity: p.quantity,
            avg_price: r2(p.average_price),
            ltp: r2(p.last_price),
            pnl: r2(p.pnl),
            m2m: r2(p.m2m),
            realised: r2(p.realised),
            unrealised: r2(p.unrealised),
            buy_qty: p.buy_quantity,
            sell_qty: p.sell_quantity,
        }
    }
}

#[tool_router(router = portfolio_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Get long-term equity holdings (demat) with per-stock and total P&L, current value,
    /// and today's change. Quantities include T1 (bought, not yet settled).
    #[tool(annotations(title = "Get holdings", read_only_hint = true, open_world_hint = true))]
    async fn get_holdings(&self, Parameters(args): Parameters<HoldingsArgs>) -> ToolResult {
        let result = self.client.holdings().await.map(|holdings| {
            let mut rows: Vec<HoldingRow> = holdings.iter().map(HoldingRow::from).collect();
            let summary = summarize(&rows);
            if let Some(f) = args.symbol.as_deref().map(str::to_ascii_uppercase) {
                rows.retain(|r| r.symbol.contains(&f));
            }
            match args.sort_by.as_deref().unwrap_or("value") {
                "pnl" => rows.sort_by(|a, b| b.pnl.total_cmp(&a.pnl)),
                "pnl_pct" => rows.sort_by(|a, b| b.pnl_pct.total_cmp(&a.pnl_pct)),
                "day_change_pct" => {
                    rows.sort_by(|a, b| b.day_change_pct.total_cmp(&a.day_change_pct))
                }
                "symbol" => rows.sort_by(|a, b| a.symbol.cmp(&b.symbol)),
                _ => rows.sort_by(|a, b| b.current_value.total_cmp(&a.current_value)),
            }
            if let Some(n) = args.limit {
                rows.truncate(n);
            }
            serde_json::json!({ "summary": summary, "holdings": rows })
        });
        self.respond(result)
    }

    /// Get open F&O / intraday positions with P&L and M2M. Defaults to net positions,
    /// excluding squared-off ones.
    #[tool(annotations(title = "Get positions", read_only_hint = true, open_world_hint = true))]
    async fn get_positions(&self, Parameters(args): Parameters<PositionsArgs>) -> ToolResult {
        let day = matches!(args.kind.as_deref(), Some("day"));
        let result = self.client.positions().await.map(|p| {
            let list = if day { &p.day } else { &p.net };
            let rows: Vec<PositionRow> = list
                .iter()
                .filter(|p| args.include_closed || p.quantity != 0)
                .map(PositionRow::from)
                .collect();
            let total_pnl: f64 = list.iter().map(|p| p.pnl).sum();
            let total_m2m: f64 = list.iter().map(|p| p.m2m).sum();
            serde_json::json!({
                "kind": if day { "day" } else { "net" },
                "total_pnl": r2(total_pnl),
                "total_m2m": r2(total_m2m),
                "open_count": list.iter().filter(|p| p.quantity != 0).count(),
                "positions": rows,
            })
        });
        self.respond(result)
    }

    /// List holdings currently eligible for exchange auctions (short-delivery auctions).
    #[tool(annotations(title = "Get auctions", read_only_hint = true, open_world_hint = true))]
    async fn get_auctions(&self) -> ToolResult {
        let result = self.client.auctions().await;
        self.respond(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holding_row_and_summary_math() {
        let h = Holding {
            tradingsymbol: "SBIN".into(),
            quantity: 10,
            t1_quantity: 2,
            average_price: 500.0,
            last_price: 550.0,
            day_change: 5.0,
            ..Default::default()
        };
        let row = HoldingRow::from(&h);
        assert_eq!(row.invested, 6000.0);
        assert_eq!(row.current_value, 6600.0);
        assert_eq!(row.pnl, 600.0);
        assert_eq!(row.pnl_pct, 10.0);
        assert_eq!(row.day_change, 60.0);

        let s = summarize(&[row]);
        assert_eq!(s.total_pnl, 600.0);
        assert_eq!(s.day_change_pct, pct(60.0, 6540.0));
    }
}
