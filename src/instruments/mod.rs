//! Instrument master: the daily `/instruments` CSV, cached on disk and indexed in memory.
//!
//! Used to resolve `EXCHANGE:SYMBOL` <-> instrument token and to search for instruments
//! (stocks, futures, options) by name, expiry and strike.

pub mod history;
pub mod search;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, NaiveTime, TimeZone, Utc};
use tokio::sync::{Mutex, RwLock};

use crate::error::{KiteError, Result};
use crate::kite::KiteClient;
use crate::kite::models::market::Instrument;

/// Kite publishes the new instrument dump around 08:30 IST.
const REFRESH_TIME_IST: (u32, u32) = (8, 30);

pub struct Index {
    pub items: Vec<Instrument>,
    by_key: HashMap<String, usize>,
    by_token: HashMap<u64, usize>,
    pub fetched_at: DateTime<Utc>,
}

impl Index {
    pub fn from_csv(csv_text: &str, fetched_at: DateTime<Utc>) -> Result<Self> {
        let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
        let mut items = Vec::new();
        for row in reader.deserialize::<Instrument>() {
            match row {
                Ok(i) => items.push(i),
                Err(e) => tracing::debug!("skipping bad instrument row: {e}"),
            }
        }
        if items.is_empty() {
            return Err(KiteError::Decode("instrument dump has no rows".into()));
        }
        let mut by_key = HashMap::with_capacity(items.len());
        let mut by_token = HashMap::with_capacity(items.len());
        for (idx, i) in items.iter().enumerate() {
            by_key.insert(key(&i.exchange, &i.tradingsymbol), idx);
            by_token.insert(i.instrument_token, idx);
        }
        Ok(Self {
            items,
            by_key,
            by_token,
            fetched_at,
        })
    }

    pub fn by_symbol(&self, exchange: &str, tradingsymbol: &str) -> Option<&Instrument> {
        self.by_key
            .get(&key(exchange, tradingsymbol))
            .map(|&i| &self.items[i])
    }

    pub fn by_token(&self, token: u64) -> Option<&Instrument> {
        self.by_token.get(&token).map(|&i| &self.items[i])
    }

    /// Resolves `EXCHANGE:SYMBOL`, a bare symbol (tries NSE, then BSE) or a numeric token.
    pub fn resolve(&self, ident: &str) -> Result<&Instrument> {
        let ident = ident.trim();
        if let Ok(token) = ident.parse::<u64>() {
            return self.by_token(token).ok_or_else(|| not_found(ident));
        }
        if let Some((exchange, symbol)) = ident.split_once(':') {
            return self
                .by_symbol(exchange, symbol)
                .ok_or_else(|| not_found(ident));
        }
        ["NSE", "BSE"]
            .iter()
            .find_map(|ex| self.by_symbol(ex, ident))
            .ok_or_else(|| not_found(ident))
    }
}

fn key(exchange: &str, tradingsymbol: &str) -> String {
    format!(
        "{}:{}",
        exchange.trim().to_ascii_uppercase(),
        tradingsymbol.trim().to_ascii_uppercase()
    )
}

fn not_found(ident: &str) -> KiteError {
    KiteError::NotFound(format!(
        "instrument `{ident}` not found. Use `search_instruments` to find the exact tradingsymbol."
    ))
}

/// Normalizes a user-supplied instrument to what the quote APIs accept:
/// `EXCHANGE:SYMBOL` (uppercased) or a numeric token. Bare symbols default to NSE.
pub fn normalize_quote_key(ident: &str) -> String {
    let ident = ident.trim();
    if ident.parse::<u64>().is_ok() {
        return ident.to_string();
    }
    match ident.split_once(':') {
        Some((ex, sym)) => key(ex, sym),
        None => key("NSE", ident),
    }
}

/// Most recent refresh boundary (08:30 IST) at or before `now`.
pub fn last_refresh_boundary(now: DateTime<Utc>) -> DateTime<Utc> {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset");
    let local = now.with_timezone(&ist);
    let t = NaiveTime::from_hms_opt(REFRESH_TIME_IST.0, REFRESH_TIME_IST.1, 0).expect("valid");
    let mut day = local.date_naive();
    if local.time() < t {
        day -= Duration::days(1);
    }
    ist.from_local_datetime(&day.and_time(t))
        .single()
        .expect("fixed offset")
        .with_timezone(&Utc)
}

pub struct InstrumentStore {
    client: Arc<KiteClient>,
    cache_path: PathBuf,
    index: RwLock<Option<Arc<Index>>>,
    refresh: Mutex<()>,
}

impl InstrumentStore {
    pub fn new(client: Arc<KiteClient>, home: PathBuf) -> Self {
        Self {
            client,
            cache_path: home.join("instruments.csv"),
            index: RwLock::new(None),
            refresh: Mutex::new(()),
        }
    }

    /// Returns a fresh index, loading from disk or downloading as needed.
    pub async fn get(&self) -> Result<Arc<Index>> {
        let boundary = last_refresh_boundary(Utc::now());
        if let Some(idx) = self.index.read().await.as_ref()
            && idx.fetched_at >= boundary
        {
            return Ok(idx.clone());
        }

        let _guard = self.refresh.lock().await;
        // Another caller may have refreshed while we waited.
        if let Some(idx) = self.index.read().await.as_ref()
            && idx.fetched_at >= boundary
        {
            return Ok(idx.clone());
        }

        let idx = match self.load_disk(boundary).await {
            Some(idx) => idx,
            None => self.download().await?,
        };
        let idx = Arc::new(idx);
        *self.index.write().await = Some(idx.clone());
        Ok(idx)
    }

    /// Forces a re-download on the next `get`.
    pub async fn invalidate(&self) {
        *self.index.write().await = None;
        let _ = tokio::fs::remove_file(&self.cache_path).await;
    }

    async fn load_disk(&self, boundary: DateTime<Utc>) -> Option<Index> {
        let meta = tokio::fs::metadata(&self.cache_path).await.ok()?;
        let modified: DateTime<Utc> = meta.modified().ok()?.into();
        if modified < boundary {
            return None;
        }
        let text = tokio::fs::read_to_string(&self.cache_path).await.ok()?;
        let idx = tokio::task::spawn_blocking(move || Index::from_csv(&text, modified))
            .await
            .ok()?
            .ok()?;
        tracing::info!("loaded {} instruments from disk cache", idx.items.len());
        Some(idx)
    }

    async fn download(&self) -> Result<Index> {
        tracing::info!("downloading instrument dump");
        let text = self.client.instruments_csv().await?;
        if let Some(dir) = self.cache_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&self.cache_path, &text).await?;
        let now = Utc::now();
        let idx = tokio::task::spawn_blocking(move || Index::from_csv(&text, now))
            .await
            .map_err(|e| KiteError::Decode(format!("parse task failed: {e}")))??;
        tracing::info!("indexed {} instruments", idx.items.len());
        Ok(idx)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SAMPLE_CSV: &str = "\
instrument_token,exchange_token,tradingsymbol,name,last_price,expiry,strike,tick_size,lot_size,instrument_type,segment,exchange
408065,1594,INFY,INFOSYS,0,,0,0.05,1,EQ,NSE,NSE
128053508,500209,INFY,INFOSYS,0,,0,0.05,1,EQ,BSE,BSE
779521,3045,SBIN,STATE BANK OF INDIA,0,,0,0.05,1,EQ,NSE,NSE
256265,1001,NIFTY 50,NIFTY 50,0,,0,0,0,EQ,INDICES,NSE
13238786,51714,NIFTY26NOVFUT,NIFTY,0,2026-11-24,0,0.1,75,FUT,NFO-FUT,NFO
13240322,51720,NIFTY26DEC24000CE,NIFTY,0,2026-12-29,24000,0.05,75,CE,NFO-OPT,NFO
13240578,51721,NIFTY26NOV24000CE,NIFTY,0,2026-11-24,24000,0.05,75,CE,NFO-OPT,NFO
13240834,51722,NIFTY26NOV24000PE,NIFTY,0,2026-11-24,24000,0.05,75,PE,NFO-OPT,NFO
";

    pub fn sample_index() -> Index {
        Index::from_csv(SAMPLE_CSV, Utc::now()).unwrap()
    }

    #[test]
    fn resolves_symbols_and_tokens() {
        let idx = sample_index();
        assert_eq!(idx.resolve("NSE:INFY").unwrap().instrument_token, 408065);
        assert_eq!(idx.resolve("bse:infy").unwrap().instrument_token, 128053508);
        assert_eq!(idx.resolve("SBIN").unwrap().exchange, "NSE");
        assert_eq!(idx.resolve("779521").unwrap().tradingsymbol, "SBIN");
        assert!(idx.resolve("NSE:NOPE").is_err());
    }

    #[test]
    fn normalizes_quote_keys() {
        assert_eq!(normalize_quote_key("infy"), "NSE:INFY");
        assert_eq!(normalize_quote_key("bse:sbin"), "BSE:SBIN");
        assert_eq!(normalize_quote_key("NSE:NIFTY 50"), "NSE:NIFTY 50");
        assert_eq!(normalize_quote_key("408065"), "408065");
    }

    #[test]
    fn refresh_boundary_is_830_ist() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        // 10:00 IST -> same day 08:30 IST (03:00 UTC)
        assert_eq!(
            last_refresh_boundary(at("2026-11-02T04:30:00Z")),
            at("2026-11-02T03:00:00Z")
        );
        // 07:00 IST -> previous day 08:30 IST
        assert_eq!(
            last_refresh_boundary(at("2026-11-02T01:30:00Z")),
            at("2026-11-01T03:00:00Z")
        );
    }
}
