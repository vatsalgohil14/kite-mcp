use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::default_limit;
use crate::error::KiteError;
use crate::kite::models::orders::Order;
use crate::mcp::server::{KiteMcp, ToolResult};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OrdersArgs {
    /// `open` (pending/trigger pending/etc.), `complete`, `cancelled`, `rejected`, or `all` (default).
    #[serde(default)]
    pub status: Option<String>,
    /// Only orders whose tradingsymbol contains this text (case-insensitive).
    #[serde(default)]
    pub symbol: Option<String>,
    /// Only orders with this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Max rows, newest first (default 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OrderIdArgs {
    pub order_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TradesArgs {
    /// Only trades whose tradingsymbol contains this text (case-insensitive).
    #[serde(default)]
    pub symbol: Option<String>,
    /// Max rows, newest first (default 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Compact order view: drops empty and rarely useful fields.
#[derive(Debug, Serialize)]
struct OrderRow<'a> {
    order_id: &'a str,
    time: Option<&'a str>,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_message: Option<&'a str>,
    variety: &'a str,
    exchange: &'a str,
    symbol: &'a str,
    side: &'a str,
    order_type: &'a str,
    product: &'a str,
    validity: &'a str,
    quantity: i64,
    filled: i64,
    pending: i64,
    price: f64,
    #[serde(skip_serializing_if = "is_zero")]
    trigger_price: f64,
    #[serde(skip_serializing_if = "is_zero")]
    average_price: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<&'a str>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

impl<'a> From<&'a Order> for OrderRow<'a> {
    fn from(o: &'a Order) -> Self {
        Self {
            order_id: &o.order_id,
            time: o.order_timestamp.as_deref(),
            status: &o.status,
            status_message: o.status_message.as_deref().filter(|s| !s.is_empty()),
            variety: &o.variety,
            exchange: &o.exchange,
            symbol: &o.tradingsymbol,
            side: &o.transaction_type,
            order_type: &o.order_type,
            product: &o.product,
            validity: &o.validity,
            quantity: o.quantity,
            filled: o.filled_quantity,
            pending: o.pending_quantity,
            price: o.price,
            trigger_price: o.trigger_price,
            average_price: o.average_price,
            tag: o.tag.as_deref(),
        }
    }
}

fn status_matches(o: &Order, filter: &str) -> Result<bool, KiteError> {
    Ok(match filter {
        "all" => true,
        "open" => o.is_open(),
        "complete" => o.status == "COMPLETE",
        "cancelled" => o.status == "CANCELLED",
        "rejected" => o.status == "REJECTED",
        other => {
            return Err(KiteError::InvalidInput(format!(
                "status must be open, complete, cancelled, rejected or all; got `{other}`"
            )));
        }
    })
}

/// Applies the order-book filters and returns newest first.
fn filter_orders<'a>(
    orders: &'a [Order],
    args: &OrdersArgs,
) -> Result<Vec<OrderRow<'a>>, KiteError> {
    let status = args.status.as_deref().unwrap_or("all").to_ascii_lowercase();
    let symbol = args.symbol.as_deref().map(str::to_ascii_uppercase);
    let mut rows = Vec::new();
    for o in orders.iter().rev() {
        if !status_matches(o, &status)? {
            continue;
        }
        if symbol
            .as_ref()
            .is_some_and(|s| !o.tradingsymbol.contains(s.as_str()))
        {
            continue;
        }
        if args
            .tag
            .as_ref()
            .is_some_and(|t| o.tag.as_deref() != Some(t.as_str()))
        {
            continue;
        }
        rows.push(OrderRow::from(o));
        if rows.len() >= args.limit {
            break;
        }
    }
    Ok(rows)
}

#[tool_router(router = orders_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Get today's order book (open and executed), newest first, with optional filters.
    #[tool(annotations(title = "Get orders", read_only_hint = true, open_world_hint = true))]
    async fn get_orders(&self, Parameters(args): Parameters<OrdersArgs>) -> ToolResult {
        let result = match self.client.orders().await {
            Ok(orders) => filter_orders(&orders, &args).map(|rows| {
                let open = orders.iter().filter(|o| o.is_open()).count();
                serde_json::json!({ "total": orders.len(), "open": open, "orders": rows })
            }),
            Err(e) => Err(e),
        };
        self.respond(result)
    }

    /// Get every status transition of one order (placed, open, modified, complete, ...),
    /// including rejection reasons.
    #[tool(annotations(
        title = "Get order history",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_order_history(&self, Parameters(args): Parameters<OrderIdArgs>) -> ToolResult {
        let result = self
            .client
            .order_history(args.order_id.trim())
            .await
            .map(|h| {
                h.iter()
                    .map(OrderRow::from)
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .collect::<Vec<_>>()
            });
        self.respond(result)
    }

    /// Get today's executed trades (fills), newest first.
    #[tool(annotations(title = "Get trades", read_only_hint = true, open_world_hint = true))]
    async fn get_trades(&self, Parameters(args): Parameters<TradesArgs>) -> ToolResult {
        let symbol = args.symbol.as_deref().map(str::to_ascii_uppercase);
        let result = self.client.trades().await.map(|mut trades| {
            if let Some(s) = symbol {
                trades.retain(|t| t.tradingsymbol.contains(&s));
            }
            trades.reverse();
            trades.truncate(args.limit);
            trades
        });
        self.respond(result)
    }

    /// Get the trades (fills) generated by one order.
    #[tool(annotations(
        title = "Get order trades",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_order_trades(&self, Parameters(args): Parameters<OrderIdArgs>) -> ToolResult {
        let result = self.client.order_trades(args.order_id.trim()).await;
        self.respond(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: &str, status: &str, sym: &str) -> Order {
        Order {
            order_id: id.into(),
            status: status.into(),
            tradingsymbol: sym.into(),
            ..Default::default()
        }
    }

    fn args(status: Option<&str>, symbol: Option<&str>, limit: usize) -> OrdersArgs {
        OrdersArgs {
            status: status.map(Into::into),
            symbol: symbol.map(Into::into),
            tag: None,
            limit,
        }
    }

    #[test]
    fn filters_and_orders_newest_first() {
        let orders = vec![
            order("1", "COMPLETE", "INFY"),
            order("2", "OPEN", "SBIN"),
            order("3", "TRIGGER PENDING", "INFY"),
            order("4", "REJECTED", "INFY"),
        ];
        let open = filter_orders(&orders, &args(Some("open"), None, 50)).unwrap();
        assert_eq!(
            open.iter().map(|r| r.order_id).collect::<Vec<_>>(),
            ["3", "2"]
        );

        let infy = filter_orders(&orders, &args(None, Some("infy"), 2)).unwrap();
        assert_eq!(
            infy.iter().map(|r| r.order_id).collect::<Vec<_>>(),
            ["4", "3"]
        );

        assert!(filter_orders(&orders, &args(Some("bogus"), None, 5)).is_err());
    }
}
