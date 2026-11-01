//! Tool groups. Each module adds one `#[tool_router]` impl block to `KiteMcp`.

mod account;
mod market;
mod orders;
mod portfolio;

/// Rounds to 2 decimals to keep tool output compact.
pub(crate) fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Percentage change from `base` to `value`, 0 when `base` is 0.
pub(crate) fn pct(delta: f64, base: f64) -> f64 {
    if base == 0.0 {
        0.0
    } else {
        r2(delta / base * 100.0)
    }
}

pub(crate) fn default_limit() -> usize {
    50
}
