use std::sync::RwLock;
use std::time::Duration;

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::ratelimit::{Bucket, RateLimits};
use crate::error::{KiteError, Result};

const KITE_VERSION: &str = "3";

/// Low-level Kite Connect v3 HTTP client. Endpoint wrappers live in `kite::api::*`.
pub struct KiteClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    access_token: RwLock<Option<String>>,
    limits: RateLimits,
}

#[derive(Deserialize)]
struct Envelope<T> {
    status: Option<String>,
    data: Option<T>,
    message: Option<String>,
    error_type: Option<String>,
}

impl KiteClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("kite-mcp/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            access_token: RwLock::new(None),
            limits: RateLimits::default(),
        })
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn set_access_token(&self, token: Option<String>) {
        *self.access_token.write().unwrap() = token;
    }

    pub fn access_token(&self) -> Option<String> {
        self.access_token.read().unwrap().clone()
    }

    fn request(&self, method: Method, path: &str, authed: bool) -> Result<RequestBuilder> {
        let mut req = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .header("X-Kite-Version", KITE_VERSION);
        if authed {
            let token = self.access_token().ok_or(KiteError::NotLoggedIn)?;
            req = req.header("Authorization", format!("token {}:{}", self.api_key, token));
        }
        Ok(req)
    }

    async fn send<T: DeserializeOwned>(&self, bucket: Bucket, req: RequestBuilder) -> Result<T> {
        self.limits.acquire(bucket).await;
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        parse_envelope(status, &body)
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        bucket: Bucket,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let req = self.request(Method::GET, path, true)?.query(query);
        self.send(bucket, req).await
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        bucket: Bucket,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<T> {
        let req = self.request(Method::POST, path, true)?.form(form);
        self.send(bucket, req).await
    }

    pub async fn put<T: DeserializeOwned>(
        &self,
        bucket: Bucket,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<T> {
        let req = self.request(Method::PUT, path, true)?.form(form);
        self.send(bucket, req).await
    }

    pub async fn delete<T: DeserializeOwned>(
        &self,
        bucket: Bucket,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let req = self.request(Method::DELETE, path, true)?.query(query);
        self.send(bucket, req).await
    }

    /// Authenticated GET returning the raw body (used for the `/instruments` CSV dump).
    pub async fn get_text(&self, bucket: Bucket, path: &str) -> Result<String> {
        let req = self.request(Method::GET, path, true)?;
        self.limits.acquire(bucket).await;
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        if status.is_success() {
            return Ok(body);
        }
        Err(parse_envelope::<serde_json::Value>(status, &body)
            .err()
            .unwrap_or_else(|| KiteError::Decode(format!("HTTP {status}"))))
    }

    /// Unauthenticated form POST (used for `/session/token`).
    pub async fn post_public<T: DeserializeOwned>(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<T> {
        let req = self.request(Method::POST, path, false)?.form(form);
        self.send(Bucket::Default, req).await
    }
}

fn parse_envelope<T: DeserializeOwned>(status: StatusCode, body: &str) -> Result<T> {
    let env: Envelope<T> = serde_json::from_str(body).map_err(|e| {
        let snippet: String = body.chars().take(200).collect();
        KiteError::Decode(format!("HTTP {status}: {e}; body: {snippet}"))
    })?;

    if status.is_success() && env.status.as_deref() != Some("error") {
        return env
            .data
            .ok_or_else(|| KiteError::Decode("response has no `data` field".into()));
    }
    Err(KiteError::Api {
        status: status.as_u16(),
        error_type: env.error_type.unwrap_or_else(|| "UnknownException".into()),
        message: env.message.unwrap_or_else(|| format!("HTTP {status}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_envelope() {
        let v: serde_json::Value =
            parse_envelope(StatusCode::OK, r#"{"status":"success","data":{"a":1}}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn parses_error_envelope() {
        let err = parse_envelope::<serde_json::Value>(
            StatusCode::FORBIDDEN,
            r#"{"status":"error","message":"Incorrect `api_key` or `access_token`.","error_type":"TokenException"}"#,
        )
        .unwrap_err();
        assert!(err.is_token_error());
        assert!(err.to_string().contains("Incorrect"));
    }

    #[test]
    fn non_json_body_is_decode_error() {
        let err =
            parse_envelope::<serde_json::Value>(StatusCode::BAD_GATEWAY, "<html>").unwrap_err();
        assert!(matches!(err, KiteError::Decode(_)));
    }
}
