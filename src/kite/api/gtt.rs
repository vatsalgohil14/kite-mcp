use serde_json::Value;

use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::ratelimit::Bucket;

impl KiteClient {
    pub async fn gtts(&self) -> Result<Vec<Value>> {
        self.get(Bucket::Default, "/gtt/triggers", &[]).await
    }

    pub async fn gtt(&self, id: u64) -> Result<Value> {
        self.get(Bucket::Default, &format!("/gtt/triggers/{id}"), &[])
            .await
    }

    /// `kind` is `single` or `two-leg`; `condition` and `orders` are JSON as Kite expects.
    pub async fn place_gtt(&self, kind: &str, condition: &Value, orders: &Value) -> Result<Value> {
        let form = gtt_form(kind, condition, orders);
        self.post(Bucket::Orders, "/gtt/triggers", &form).await
    }

    pub async fn modify_gtt(
        &self,
        id: u64,
        kind: &str,
        condition: &Value,
        orders: &Value,
    ) -> Result<Value> {
        let form = gtt_form(kind, condition, orders);
        self.put(Bucket::Orders, &format!("/gtt/triggers/{id}"), &form)
            .await
    }

    pub async fn delete_gtt(&self, id: u64) -> Result<Value> {
        self.delete(Bucket::Orders, &format!("/gtt/triggers/{id}"), &[])
            .await
    }
}

fn gtt_form(kind: &str, condition: &Value, orders: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("type", kind.to_string()),
        ("condition", condition.to_string()),
        ("orders", orders.to_string()),
    ]
}
