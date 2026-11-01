//! Endpoints that change state: orders, position conversion, holdings authorisation,
//! plus the margin and charge calculators used to preview them.

use serde::Deserialize;
use serde_json::Value;

use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::ratelimit::Bucket;

pub type Form = Vec<(&'static str, String)>;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OrderId {
    pub order_id: String,
}

impl KiteClient {
    pub async fn place_order(&self, variety: &str, form: &Form) -> Result<OrderId> {
        self.post(Bucket::Orders, &format!("/orders/{variety}"), form)
            .await
    }

    pub async fn modify_order(
        &self,
        variety: &str,
        order_id: &str,
        form: &Form,
    ) -> Result<OrderId> {
        self.put(
            Bucket::Orders,
            &format!("/orders/{variety}/{order_id}"),
            form,
        )
        .await
    }

    pub async fn cancel_order(
        &self,
        variety: &str,
        order_id: &str,
        parent_order_id: Option<&str>,
    ) -> Result<OrderId> {
        let query: Vec<(&str, String)> = parent_order_id
            .map(|p| vec![("parent_order_id", p.to_string())])
            .unwrap_or_default();
        self.delete(
            Bucket::Orders,
            &format!("/orders/{variety}/{order_id}"),
            &query,
        )
        .await
    }

    pub async fn convert_position(&self, form: &Form) -> Result<bool> {
        self.put(Bucket::Default, "/portfolio/positions", form)
            .await
    }

    /// Starts CDSL authorisation for selling holdings. Returns a `request_id`.
    pub async fn authorise_holdings(&self, items: &[(String, i64)]) -> Result<Value> {
        let form: Vec<(&str, String)> = items
            .iter()
            .flat_map(|(isin, qty)| [("isin", isin.clone()), ("quantity", qty.to_string())])
            .collect();
        self.post(Bucket::Default, "/portfolio/holdings/authorise", &form)
            .await
    }

    pub async fn order_margins(&self, orders: &Value, compact: bool) -> Result<Value> {
        let q = compact_query(compact);
        self.post_json(Bucket::Default, "/margins/orders", &q, orders)
            .await
    }

    pub async fn basket_margins(
        &self,
        orders: &Value,
        consider_positions: bool,
        compact: bool,
    ) -> Result<Value> {
        let mut q = compact_query(compact);
        q.push(("consider_positions", consider_positions.to_string()));
        self.post_json(Bucket::Default, "/margins/basket", &q, orders)
            .await
    }

    pub async fn order_charges(&self, orders: &Value) -> Result<Value> {
        self.post_json(Bucket::Default, "/charges/orders", &[], orders)
            .await
    }
}

fn compact_query(compact: bool) -> Vec<(&'static str, String)> {
    if compact {
        vec![("mode", "compact".into())]
    } else {
        Vec::new()
    }
}
