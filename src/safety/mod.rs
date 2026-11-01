//! Guards for actions that move money: two-step confirmation, dry-run, optional
//! exchange/product/symbol lists and an append-only audit log. There are no order-value,
//! quantity or daily-count limits.

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

/// One order leg as seen by the checks.
#[derive(Debug, Clone)]
pub struct OrderCheck<'a> {
    pub exchange: &'a str,
    pub tradingsymbol: &'a str,
    pub product: &'a str,
    pub quantity: i64,
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

    /// Validates one order leg: positive quantity, plus the user's own exchange/product/
    /// symbol lists when configured (all empty by default, so nothing is restricted).
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safety(cfg: SafetyConfig) -> Safety {
        Safety::new(cfg, tempfile::tempdir().unwrap().keep())
    }

    fn order(qty: i64) -> OrderCheck<'static> {
        OrderCheck {
            exchange: "NSE",
            tradingsymbol: "INFY",
            product: "CNC",
            quantity: qty,
        }
    }

    #[test]
    fn no_value_or_quantity_limits_by_default() {
        let s = safety(SafetyConfig::default());
        assert!(s.check_order(&order(1_000_000)).is_ok());
        assert!(s.check_order(&order(0)).is_err());
    }

    #[test]
    fn allow_and_block_lists() {
        let s = safety(SafetyConfig {
            allowed_exchanges: vec!["BSE".into()],
            ..Default::default()
        });
        assert!(s.check_order(&order(1)).is_err());

        let s = safety(SafetyConfig {
            allowed_products: vec!["MIS".into()],
            ..Default::default()
        });
        assert!(s.check_order(&order(1)).is_err());

        let s = safety(SafetyConfig {
            blocked_symbols: vec!["infy".into()],
            ..Default::default()
        });
        assert!(s.check_order(&order(1)).is_err());
    }
}
