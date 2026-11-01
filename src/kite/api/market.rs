use serde::de::DeserializeOwned;

use crate::error::Result;
use crate::kite::client::KiteClient;
use crate::kite::models::market::{HistoricalData, LtpQuote, OhlcQuote, Quote, QuoteMap};
use crate::kite::ratelimit::Bucket;

/// Max instruments per request for `/quote` and `/quote/ohlc`.
pub const QUOTE_BATCH: usize = 500;
/// Max instruments per request for `/quote/ltp`.
pub const LTP_BATCH: usize = 1000;

impl KiteClient {
    /// Full quotes (with market depth) for `EXCHANGE:SYMBOL` keys or instrument tokens.
    pub async fn quote(&self, instruments: &[String]) -> Result<QuoteMap<Quote>> {
        self.batched_quote("/quote", instruments, QUOTE_BATCH).await
    }

    pub async fn ohlc(&self, instruments: &[String]) -> Result<QuoteMap<OhlcQuote>> {
        self.batched_quote("/quote/ohlc", instruments, QUOTE_BATCH)
            .await
    }

    pub async fn ltp(&self, instruments: &[String]) -> Result<QuoteMap<LtpQuote>> {
        self.batched_quote("/quote/ltp", instruments, LTP_BATCH)
            .await
    }

    async fn batched_quote<T: DeserializeOwned>(
        &self,
        path: &str,
        instruments: &[String],
        batch: usize,
    ) -> Result<QuoteMap<T>> {
        let mut out = QuoteMap::new();
        for chunk in instruments.chunks(batch) {
            let query: Vec<(&str, String)> = chunk.iter().map(|i| ("i", i.clone())).collect();
            let part: QuoteMap<T> = self.get(Bucket::Quote, path, &query).await?;
            out.extend(part);
        }
        Ok(out)
    }

    /// Raw candles for one request window. `from`/`to` are `YYYY-MM-DD HH:MM:SS` (IST).
    /// Callers must respect the per-interval window limits (see `instruments::history`).
    pub async fn historical(
        &self,
        instrument_token: u64,
        interval: &str,
        from: &str,
        to: &str,
        continuous: bool,
        oi: bool,
    ) -> Result<HistoricalData> {
        let query = [
            ("from", from.to_string()),
            ("to", to.to_string()),
            ("continuous", u8::from(continuous).to_string()),
            ("oi", u8::from(oi).to_string()),
        ];
        let path = format!("/instruments/historical/{instrument_token}/{interval}");
        self.get(Bucket::Historical, &path, &query).await
    }

    /// The full instruments CSV dump (all exchanges), ~100k rows.
    pub async fn instruments_csv(&self) -> Result<String> {
        self.get_text(Bucket::Default, "/instruments").await
    }
}
