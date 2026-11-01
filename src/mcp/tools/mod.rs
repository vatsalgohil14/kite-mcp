//! Tool groups. Each module adds one or more `#[tool_router]` impl blocks to `KiteMcp`.

mod account;
mod alerts;
mod gtt;
mod insights;
mod live;
mod margins;
mod market;
pub(crate) mod mf;
mod orders;
mod portfolio;
pub(crate) mod trading;

/// Rounds to 2 decimals to keep tool output compact.
pub(crate) fn r2(x: f64) -> f64 {
    // `+ 0.0` turns -0.0 into 0.0.
    (x * 100.0).round() / 100.0 + 0.0
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
