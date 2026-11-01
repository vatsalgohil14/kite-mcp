use std::num::NonZeroU32;

use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};

/// Kite Connect rate-limit buckets.
/// https://kite.trade/docs/connect/v3/exceptions/#api-rate-limit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Quote,
    Historical,
    Orders,
    Default,
}

pub struct RateLimits {
    quote: DefaultDirectRateLimiter,
    historical: DefaultDirectRateLimiter,
    orders: DefaultDirectRateLimiter,
    default: DefaultDirectRateLimiter,
}

fn per_second(n: u32) -> DefaultDirectRateLimiter {
    RateLimiter::direct(Quota::per_second(
        NonZeroU32::new(n).expect("non-zero rate"),
    ))
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            quote: per_second(1),
            historical: per_second(3),
            orders: per_second(10),
            default: per_second(10),
        }
    }
}

impl RateLimits {
    /// Waits until a request in `bucket` is allowed.
    pub async fn acquire(&self, bucket: Bucket) {
        let limiter = match bucket {
            Bucket::Quote => &self.quote,
            Bucket::Historical => &self.historical,
            Bucket::Orders => &self.orders,
            Bucket::Default => &self.default,
        };
        limiter.until_ready().await;
    }
}
