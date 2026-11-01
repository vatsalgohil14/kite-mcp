use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::models::orders::{Order, Trade};
use crate::kite::ratelimit::Bucket;

impl KiteClient {
    /// All orders (open and executed) for the day.
    pub async fn orders(&self) -> Result<Vec<Order>> {
        self.get(Bucket::Default, "/orders", &[]).await
    }

    /// Every state transition of one order, oldest first.
    pub async fn order_history(&self, order_id: &str) -> Result<Vec<Order>> {
        self.get(Bucket::Default, &format!("/orders/{order_id}"), &[])
            .await
    }

    /// All executed trades for the day.
    pub async fn trades(&self) -> Result<Vec<Trade>> {
        self.get(Bucket::Default, "/trades", &[]).await
    }

    pub async fn order_trades(&self, order_id: &str) -> Result<Vec<Trade>> {
        self.get(Bucket::Default, &format!("/orders/{order_id}/trades"), &[])
            .await
    }
}
