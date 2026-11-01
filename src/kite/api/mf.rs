//! Mutual funds (read-only: Kite Connect no longer documents MF order placement).

use serde_json::Value;

use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::ratelimit::Bucket;

impl KiteClient {
    /// MF orders from the last 7 days.
    pub async fn mf_orders(&self) -> Result<Vec<Value>> {
        self.get(Bucket::Default, "/mf/orders", &[]).await
    }

    pub async fn mf_order(&self, order_id: &str) -> Result<Value> {
        self.get(Bucket::Default, &format!("/mf/orders/{order_id}"), &[])
            .await
    }

    pub async fn mf_sips(&self) -> Result<Vec<Value>> {
        self.get(Bucket::Default, "/mf/sips", &[]).await
    }

    pub async fn mf_holdings(&self) -> Result<Vec<Value>> {
        self.get(Bucket::Default, "/mf/holdings", &[]).await
    }

    /// CSV master of all mutual funds.
    pub async fn mf_instruments_csv(&self) -> Result<String> {
        self.get_text(Bucket::Default, "/mf/instruments").await
    }
}
