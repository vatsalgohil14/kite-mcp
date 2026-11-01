//! Guards for actions that move money: limits, two-step confirmation, dry-run and an
//! append-only audit log.

mod audit;
mod confirm;

use std::path::PathBuf;

pub use audit::{AuditLog, Outcome};
pub use confirm::{Confirmations, PendingAction};

pub fn confirmations_ttl_secs() -> u64 {
    confirm::CONFIRM_TTL.as_secs()
}

use crate::config::SafetyConfig;
use crate::error::{KiteError, Result};

/// One order leg as seen by the limit checks.
#[derive(Debug, Clone)]
pub struct OrderCheck<'a> {
    pub exchange: &'a str,
    pub tradingsymbol: &'a str,
    pub product: &'a str,
    pub quantity: i64,
    /// Price used for the value check: limit/trigger price, or LTP for market orders.
    pub reference_price: Option<f64>,
}

pub struct Safety {
    pub config: SafetyConfig,
    pub confirmations: Confirmations,
    pub audit: AuditLog,
}

impl Safety {
    pub fn new(config: SafetyConfig, home: PathBuf) -> Self {
        Self {
            config,
            confirmations: Confirmations::default(),
            audit: AuditLog::new(home.join("audit.jsonl")),
        }
    }

    /// Validates one order leg against the configured limits.
    pub fn check_order(&self, o: &OrderCheck) -> Result<()> {
        let c = &self.config;
        let deny = |msg: String| Err(KiteError::Blocked(msg));

        if o.quantity <= 0 {
            return deny("quantity must be positive".into());
        }
        if !c.allowed_exchanges.is_empty()
            && !c
                .allowed_exchanges
                .iter()
                .any(|e| e.eq_ignore_ascii_case(o.exchange))
        {
            return deny(format!(
                "exchange {} is not in allowed_exchanges {:?}",
                o.exchange, c.allowed_exchanges
            ));
        }
        if !c.allowed_products.is_empty()
            && !c
                .allowed_products
                .iter()
                .any(|p| p.eq_ignore_ascii_case(o.product))
        {
            return deny(format!(
                "product {} is not in allowed_products {:?}",
                o.product, c.allowed_products
            ));
        }
        if c.blocked_symbols
            .iter()
            .any(|s| s.eq_ignore_ascii_case(o.tradingsymbol))
        {
            return deny(format!("{} is in blocked_symbols", o.tradingsymbol));
        }
        if c.max_quantity > 0 && o.quantity > c.max_quantity {
            return deny(format!(
                "quantity {} exceeds max_quantity {}",
                o.quantity, c.max_quantity
            ));
        }
        if c.max_order_value > 0.0 {
            let Some(price) = o.reference_price.filter(|p| *p > 0.0) else {
                return deny(
                    "cannot verify the order value against max_order_value: no price and no \
                     live LTP available. Use a LIMIT order (or SL with a trigger price)."
                        .into(),
                );
            };
            let value = price * o.quantity as f64;
            if value > c.max_order_value {
                return deny(format!(
                    "order value ₹{value:.2} exceeds max_order_value ₹{:.2}",
                    c.max_order_value
                ));
            }
        }
        Ok(())
    }

    /// Fails if today's order count (from the audit log) has reached the daily cap.
    pub fn check_daily_count(&self) -> Result<()> {
        let max = self.config.max_orders_per_day;
        if max == 0 {
            return Ok(());
        }
        let placed = self.audit.orders_placed_today();
        if placed >= max {
            return Err(KiteError::Blocked(format!(
                "max_orders_per_day ({max}) reached; {placed} orders placed today"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safety(cfg: SafetyConfig) -> Safety {
        Safety::new(cfg, tempfile::tempdir().unwrap().keep())
    }

    fn order(qty: i64, price: Option<f64>) -> OrderCheck<'static> {
        OrderCheck {
            exchange: "NSE",
            tradingsymbol: "INFY",
            product: "CNC",
            quantity: qty,
            reference_price: price,
        }
    }

    #[test]
    fn value_limit() {
        let s = safety(SafetyConfig::default()); // 25k
        assert!(s.check_order(&order(10, Some(1500.0))).is_ok());
        let err = s.check_order(&order(20, Some(1500.0))).unwrap_err();
        assert!(err.to_string().contains("max_order_value"));
        assert!(s.check_order(&order(1, None)).is_err());
    }

    #[test]
    fn allow_and_block_lists() {
        let s = safety(SafetyConfig {
            allowed_exchanges: vec!["BSE".into()],
            ..Default::default()
        });
        assert!(s.check_order(&order(1, Some(10.0))).is_err());

        let s = safety(SafetyConfig {
            allowed_products: vec!["MIS".into()],
            ..Default::default()
        });
        assert!(s.check_order(&order(1, Some(10.0))).is_err());

        let s = safety(SafetyConfig {
            blocked_symbols: vec!["infy".into()],
            ..Default::default()
        });
        assert!(s.check_order(&order(1, Some(10.0))).is_err());
    }

    #[test]
    fn quantity_limits() {
        let s = safety(SafetyConfig {
            max_quantity: 5,
            max_order_value: 0.0,
            ..Default::default()
        });
        assert!(s.check_order(&order(5, None)).is_ok());
        assert!(s.check_order(&order(6, None)).is_err());
        assert!(s.check_order(&order(0, None)).is_err());
    }
}
