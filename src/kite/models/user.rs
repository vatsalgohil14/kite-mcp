use serde::{Deserialize, Serialize};

/// Response of `POST /session/token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub email: String,
    pub access_token: String,
    #[serde(default)]
    pub public_token: String,
    #[serde(default)]
    pub login_time: Option<String>,
}

/// Funds and margins for one segment (`equity` or `commodity`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SegmentMargins {
    pub enabled: bool,
    pub net: f64,
    pub available: serde_json::Map<String, serde_json::Value>,
    pub utilised: serde_json::Map<String, serde_json::Value>,
}

/// Response of `GET /user/margins`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Margins {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equity: Option<SegmentMargins>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commodity: Option<SegmentMargins>,
}

/// Response of `GET /user/profile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_shortname: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub user_type: String,
    #[serde(default)]
    pub broker: String,
    #[serde(default)]
    pub exchanges: Vec<String>,
    #[serde(default)]
    pub products: Vec<String>,
    #[serde(default)]
    pub order_types: Vec<String>,
    #[serde(default)]
    pub meta: serde_json::Value,
}
