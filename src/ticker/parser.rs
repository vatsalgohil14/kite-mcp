//! Parser for Kite's binary market-data frames.
//! https://kite.trade/docs/connect/v3/websocket/#binary-market-data

use serde::Serialize;

use crate::kite::models::market::{Depth, DepthLevel, Ohlc};

const SEGMENT_CDS: u32 = 3;
const SEGMENT_BCD: u32 = 6;
const SEGMENT_INDICES: u32 = 9;

#[derive(Debug, Clone, Serialize)]
pub struct Tick {
    pub instrument_token: u32,
    pub mode: &'static str,
    pub tradable: bool,
    pub last_price: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_quantity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_price: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buy_quantity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sell_quantity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ohlc: Option<Ohlc>,
    /// % change from the previous close.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_pct: Option<f64>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_trade_time: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oi: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oi_day_high: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oi_day_low: Option<u32>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange_timestamp: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<Depth>,
}

fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be_i32(b: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be_u16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

/// Price divisor by segment: paise for most, more decimals for currency.
fn divisor(token: u32) -> f64 {
    match token & 0xff {
        SEGMENT_CDS => 10_000_000.0,
        SEGMENT_BCD => 10_000.0,
        _ => 100.0,
    }
}

fn change_pct(last: f64, close: f64) -> Option<f64> {
    (close != 0.0).then(|| ((last - close) / close * 100.0 * 100.0).round() / 100.0)
}

/// Splits a frame into packets and parses each. Heartbeats (1 byte) yield nothing;
/// malformed packets are skipped.
pub fn parse_frame(frame: &[u8]) -> Vec<Tick> {
    if frame.len() < 2 {
        return Vec::new();
    }
    let count = be_u16(frame, 0) as usize;
    let mut ticks = Vec::with_capacity(count);
    let mut at = 2;
    for _ in 0..count {
        if at + 2 > frame.len() {
            break;
        }
        let len = be_u16(frame, at) as usize;
        at += 2;
        if at + len > frame.len() {
            break;
        }
        if let Some(t) = parse_packet(&frame[at..at + len]) {
            ticks.push(t);
        }
        at += len;
    }
    ticks
}

pub fn parse_packet(p: &[u8]) -> Option<Tick> {
    if p.len() < 8 {
        return None;
    }
    let token = be_u32(p, 0);
    let d = divisor(token);
    let price = |at: usize| be_i32(p, at) as f64 / d;
    let index = token & 0xff == SEGMENT_INDICES;
    let mut t = Tick {
        instrument_token: token,
        mode: "ltp",
        tradable: !index,
        last_price: price(4),
        last_quantity: None,
        average_price: None,
        volume: None,
        buy_quantity: None,
        sell_quantity: None,
        ohlc: None,
        change_pct: None,
        last_trade_time: None,
        oi: None,
        oi_day_high: None,
        oi_day_low: None,
        exchange_timestamp: None,
        depth: None,
    };

    match (index, p.len()) {
        (_, 8) => {}
        (true, 28) | (true, 32) => {
            let ohlc = Ohlc {
                high: price(8),
                low: price(12),
                open: price(16),
                close: price(20),
            };
            t.mode = if p.len() == 32 { "full" } else { "quote" };
            t.change_pct = change_pct(t.last_price, ohlc.close);
            t.ohlc = Some(ohlc);
            if p.len() == 32 {
                t.exchange_timestamp = Some(be_u32(p, 28));
            }
        }
        (false, 44) | (false, 184) => {
            t.mode = if p.len() == 184 { "full" } else { "quote" };
            t.last_quantity = Some(be_u32(p, 8));
            t.average_price = Some(price(12));
            t.volume = Some(be_u32(p, 16));
            t.buy_quantity = Some(be_u32(p, 20));
            t.sell_quantity = Some(be_u32(p, 24));
            let ohlc = Ohlc {
                open: price(28),
                high: price(32),
                low: price(36),
                close: price(40),
            };
            t.change_pct = change_pct(t.last_price, ohlc.close);
            t.ohlc = Some(ohlc);
            if p.len() == 184 {
                t.last_trade_time = Some(be_u32(p, 44));
                t.oi = Some(be_u32(p, 48));
                t.oi_day_high = Some(be_u32(p, 52));
                t.oi_day_low = Some(be_u32(p, 56));
                t.exchange_timestamp = Some(be_u32(p, 60));
                let level = |at: usize| DepthLevel {
                    quantity: be_u32(p, at) as i64,
                    price: price(at + 4),
                    orders: be_u16(p, at + 8) as i64,
                };
                t.depth = Some(Depth {
                    buy: (0..5).map(|i| level(64 + i * 12)).collect(),
                    sell: (0..5).map(|i| level(124 + i * 12)).collect(),
                });
            }
        }
        _ => return None,
    }
    Some(t)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn packet(fields: &[i32]) -> Vec<u8> {
        fields.iter().flat_map(|f| f.to_be_bytes()).collect()
    }

    pub fn frame(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut out = (packets.len() as u16).to_be_bytes().to_vec();
        for p in packets {
            out.extend((p.len() as u16).to_be_bytes());
            out.extend(p);
        }
        out
    }

    /// INFY (408065, NSE segment 1) in full mode.
    pub fn full_packet() -> Vec<u8> {
        let mut f = vec![
            408065,
            141295,
            5,
            141247,
            7_360_198,
            0,
            5191,
            139600,
            142175,
            139555,
            138965,
            1_700_000_000,
            0,
            0,
            0,
            1_700_000_001,
        ];
        for i in 0..5 {
            f.extend([10 + i, 141290 - i * 5]);
            f.push(i32::from_be_bytes([0, 3, 0, 0])); // orders=3 + padding
        }
        for i in 0..5 {
            f.extend([20 + i, 141300 + i * 5]);
            f.push(i32::from_be_bytes([0, 4, 0, 0]));
        }
        packet(&f)
    }

    #[test]
    fn parses_ltp_and_quote_packets() {
        let ltp = packet(&[408065, 141295]);
        let quote = packet(&[
            779521, 80000, 1, 79950, 100, 2, 3, 79000, 81000, 78500, 79000,
        ]);
        let ticks = parse_frame(&frame(&[ltp, quote]));
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].mode, "ltp");
        assert_eq!(ticks[0].last_price, 1412.95);
        assert_eq!(ticks[1].mode, "quote");
        assert_eq!(ticks[1].ohlc.unwrap().high, 810.0);
        assert_eq!(ticks[1].change_pct, Some(1.27));
    }

    #[test]
    fn parses_full_packet_with_depth() {
        let t = parse_frame(&frame(&[full_packet()])).remove(0);
        assert_eq!(t.mode, "full");
        assert_eq!(t.volume, Some(7_360_198));
        assert_eq!(t.exchange_timestamp, Some(1_700_000_001));
        let depth = t.depth.unwrap();
        assert_eq!(depth.buy[0].price, 1412.9);
        assert_eq!(depth.buy[0].orders, 3);
        assert_eq!(depth.sell[4].quantity, 24);
    }

    #[test]
    fn parses_index_and_currency_prices() {
        // NIFTY 50 token 256265 is segment 9 (indices).
        let idx = packet(&[
            256265, 2_400_000, 2_410_000, 2_390_000, 2_395_000, 2_380_000, 20_000,
        ]);
        let t = parse_packet(&idx).unwrap();
        assert!(!t.tradable);
        assert_eq!(t.mode, "quote");
        assert_eq!(t.ohlc.unwrap().open, 23950.0);

        // Segment 3 (CDS) prices have 7 implied decimals.
        let cds = packet(&[(412675 & !0xff) | 3, 727_550_000]);
        assert_eq!(parse_packet(&cds).unwrap().last_price, 72.755);
    }

    #[test]
    fn ignores_heartbeats_and_garbage() {
        assert!(parse_frame(&[0]).is_empty());
        assert!(parse_frame(&frame(&[vec![1, 2, 3]])).is_empty());
        // Count says 2 but only one packet present.
        let mut f = frame(&[packet(&[408065, 100])]);
        f[1] = 2;
        assert_eq!(parse_frame(&f).len(), 1);
    }
}
