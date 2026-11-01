use serde_json::Value;

use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::ratelimit::Bucket;

impl KiteClient {
    pub async fn alerts(&self, query: &[(&str, String)]) -> Result<Value> {
        self.get(Bucket::Default, "/alerts", query).await
    }

    pub async fn alert(&self, uuid: &str) -> Result<Value> {
        self.get(Bucket::Default, &format!("/alerts/{uuid}"), &[])
            .await
    }

    pub async fn alert_history(&self, uuid: &str) -> Result<Value> {
        self.get(Bucket::Default, &format!("/alerts/{uuid}/history"), &[])
            .await
    }

    pub async fn create_alert(&self, form: &[(&str, String)]) -> Result<Value> {
        self.post(Bucket::Default, "/alerts", form).await
    }

    pub async fn modify_alert(&self, uuid: &str, form: &[(&str, String)]) -> Result<Value> {
        self.put(Bucket::Default, &format!("/alerts/{uuid}"), form)
            .await
    }

    pub async fn delete_alerts(&self, uuids: &[String]) -> Result<Value> {
        let query: Vec<(&str, String)> = uuids.iter().map(|u| ("uuid", u.clone())).collect();
        self.delete(Bucket::Default, "/alerts", &query).await
    }
}
