use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::models::user::{Margins, Profile, SegmentMargins, SessionData};
use crate::kite::ratelimit::Bucket;

/// `sha256(api_key + request_token + api_secret)` as lowercase hex.
pub fn checksum(api_key: &str, request_token: &str, api_secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(api_key.as_bytes());
    h.update(request_token.as_bytes());
    h.update(api_secret.as_bytes());
    hex::encode(h.finalize())
}

impl KiteClient {
    /// Exchanges a `request_token` from the login redirect for an access token.
    pub async fn generate_session(
        &self,
        request_token: &str,
        api_secret: &str,
    ) -> Result<SessionData> {
        let form = [
            ("api_key", self.api_key().to_string()),
            ("request_token", request_token.to_string()),
            (
                "checksum",
                checksum(self.api_key(), request_token, api_secret),
            ),
        ];
        self.post_public("/session/token", &form).await
    }

    /// Invalidates the current access token on Kite's side.
    pub async fn invalidate_session(&self) -> Result<bool> {
        let token = self.access_token().unwrap_or_default();
        let query = [
            ("api_key", self.api_key().to_string()),
            ("access_token", token),
        ];
        self.delete(Bucket::Default, "/session/token", &query).await
    }

    pub async fn profile(&self) -> Result<Profile> {
        self.get(Bucket::Default, "/user/profile", &[]).await
    }

    /// Funds and margins for all segments.
    pub async fn margins(&self) -> Result<Margins> {
        self.get(Bucket::Default, "/user/margins", &[]).await
    }

    /// Funds and margins for one segment: `equity` or `commodity`.
    pub async fn segment_margins(&self, segment: &str) -> Result<SegmentMargins> {
        self.get(Bucket::Default, &format!("/user/margins/{segment}"), &[])
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_sha256_hex() {
        // sha256("abc")
        assert_eq!(
            checksum("a", "b", "c"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
