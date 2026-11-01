use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::models::portfolio::{Holding, Positions};
use crate::kite::ratelimit::Bucket;

impl KiteClient {
    pub async fn holdings(&self) -> Result<Vec<Holding>> {
        self.get(Bucket::Default, "/portfolio/holdings", &[]).await
    }

    pub async fn positions(&self) -> Result<Positions> {
        self.get(Bucket::Default, "/portfolio/positions", &[]).await
    }

    /// Holdings currently up for auction. Kept untyped; rarely used.
    pub async fn auctions(&self) -> Result<serde_json::Value> {
        self.get(Bucket::Default, "/portfolio/holdings/auctions", &[])
            .await
    }
}
