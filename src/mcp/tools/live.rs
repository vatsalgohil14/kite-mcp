//! Live ticks over WebSocket and real-time order updates.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{KiteError, Result};
use crate::mcp::server::{KiteMcp, ToolResult};
use crate::ticker::Mode;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubscribeArgs {
    /// EXCHANGE:TRADINGSYMBOL keys (e.g. NSE:INFY, "NSE:NIFTY 50"), bare NSE symbols, or tokens.
    pub instruments: Vec<String>,
    /// ltp, quote (default; OHLC, volume, buy/sell qty) or full (adds OI, timestamps, depth).
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TicksArgs {
    /// Instruments to read; omit for all subscribed.
    #[serde(default)]
    pub instruments: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OrderUpdatesArgs {
    /// Most recent N updates (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

impl KiteMcp {
    async fn tokens_for(&self, instruments: &[String]) -> Result<Vec<(u32, String)>> {
        let idx = self.instruments.get().await?;
        instruments
            .iter()
            .map(|i| {
                let inst = idx.resolve(i)?;
                let token = u32::try_from(inst.instrument_token)
                    .map_err(|_| KiteError::InvalidInput(format!("bad token for {i}")))?;
                Ok((token, format!("{}:{}", inst.exchange, inst.tradingsymbol)))
            })
            .collect()
    }

    async fn label(&self, token: u32) -> String {
        match self.instruments.get().await {
            Ok(idx) => idx
                .by_token(token as u64)
                .map(|i| format!("{}:{}", i.exchange, i.tradingsymbol))
                .unwrap_or_else(|| token.to_string()),
            Err(_) => token.to_string(),
        }
    }
}

#[tool_router(router = live_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Start streaming live prices for instruments over Kite's WebSocket (needs the paid
    /// market-data plan). Then read them with `get_live_ticks`. Up to 3000 instruments.
    #[tool(annotations(
        title = "Subscribe to live ticks",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn subscribe_ticks(&self, Parameters(args): Parameters<SubscribeArgs>) -> ToolResult {
        let result: Result<Value> = async {
            if args.instruments.is_empty() {
                return Err(KiteError::InvalidInput("pass at least one instrument".into()));
            }
            let mode = Mode::parse(args.mode.as_deref().unwrap_or("quote"))?;
            let pairs = self.tokens_for(&args.instruments).await?;
            let tokens: Vec<u32> = pairs.iter().map(|(t, _)| *t).collect();
            self.ticker.subscribe(&tokens, mode)?;
            Ok(json!({
                "subscribed": pairs.iter().map(|(_, k)| k).collect::<Vec<_>>(),
                "mode": mode,
                "note": "Ticks arrive within a second during market hours; read them with get_live_ticks.",
            }))
        }
        .await;
        self.respond(result)
    }

    /// Stop streaming the given instruments, or all of them when none are given.
    #[tool(annotations(title = "Unsubscribe live ticks", read_only_hint = true))]
    async fn unsubscribe_ticks(&self, Parameters(args): Parameters<TicksArgs>) -> ToolResult {
        let result = async {
            let tokens: Vec<u32> = if args.instruments.is_empty() {
                Vec::new()
            } else {
                self.tokens_for(&args.instruments)
                    .await?
                    .into_iter()
                    .map(|(t, _)| t)
                    .collect()
            };
            Ok(json!({ "unsubscribed": self.ticker.unsubscribe(&tokens) }))
        }
        .await;
        self.respond(result)
    }

    /// Latest streamed tick per subscribed instrument (price, OHLC, volume, OI, depth
    /// depending on mode), with the time it was received.
    #[tool(annotations(title = "Get live ticks", read_only_hint = true))]
    async fn get_live_ticks(&self, Parameters(args): Parameters<TicksArgs>) -> ToolResult {
        let result = async {
            let tokens: Vec<u32> = if args.instruments.is_empty() {
                Vec::new()
            } else {
                self.tokens_for(&args.instruments).await?.into_iter().map(|(t, _)| t).collect()
            };
            let mut out = serde_json::Map::new();
            for (tick, at) in self.ticker.ticks(&tokens) {
                let key = self.label(tick.instrument_token).await;
                let mut v = serde_json::to_value(&tick).unwrap_or_default();
                v["received_at"] = at.into();
                out.insert(key, v);
            }
            let status = self.ticker.status();
            Ok::<_, KiteError>(json!({ "connected": status.connected, "ticks": out, "last_error": status.last_error }))
        }
        .await;
        self.respond(result)
    }

    /// Ticker connection status and current subscriptions.
    #[tool(annotations(title = "Ticker status", read_only_hint = true))]
    async fn ticker_status(&self) -> ToolResult {
        let mut subs = Vec::new();
        for (t, m) in self.ticker.subscriptions() {
            subs.push(json!({ "instrument": self.label(t).await, "token": t, "mode": m }));
        }
        self.respond(Ok(
            json!({ "status": self.ticker.status(), "subscriptions": subs }),
        ))
    }

    /// Real-time order status updates (fills, rejections, cancellations) received over the
    /// WebSocket while subscribed to any instrument.
    #[tool(annotations(title = "Order updates", read_only_hint = true))]
    async fn get_order_updates(
        &self,
        Parameters(args): Parameters<OrderUpdatesArgs>,
    ) -> ToolResult {
        self.respond(Ok(self.ticker.order_updates(args.limit.unwrap_or(20))))
    }
}
