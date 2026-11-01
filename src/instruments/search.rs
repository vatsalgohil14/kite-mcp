use super::Index;
use crate::kite::models::market::Instrument;

#[derive(Debug, Default, Clone)]
pub struct SearchQuery {
    /// Matched against tradingsymbol and name, case-insensitively.
    pub text: String,
    pub exchange: Option<String>,
    /// EQ, FUT, CE, PE.
    pub instrument_type: Option<String>,
    pub segment: Option<String>,
    /// Exact `YYYY-MM-DD`.
    pub expiry: Option<String>,
    pub strike: Option<f64>,
    pub limit: usize,
}

/// Lower is better.
fn rank(i: &Instrument, text: &str) -> Option<u8> {
    if text.is_empty() {
        return Some(4);
    }
    let sym = i.tradingsymbol.to_ascii_uppercase();
    let name = i.name.to_ascii_uppercase();
    if sym == text {
        Some(0)
    } else if name == text {
        Some(1)
    } else if sym.starts_with(text) {
        Some(2)
    } else if name.starts_with(text) {
        Some(3)
    } else if sym.contains(text) || name.contains(text) {
        Some(4)
    } else {
        None
    }
}

fn eq_ci(a: &str, b: &Option<String>) -> bool {
    b.as_ref().is_none_or(|b| a.eq_ignore_ascii_case(b.trim()))
}

/// Ranks matches: exact symbol, exact name, symbol prefix, name prefix, substring.
/// Within a rank, cash (EQ) comes before derivatives, then nearer expiry, then strike.
pub fn search<'a>(idx: &'a Index, q: &SearchQuery) -> Vec<&'a Instrument> {
    let text = q.text.trim().to_ascii_uppercase();
    let mut hits: Vec<(u8, &Instrument)> = idx
        .items
        .iter()
        .filter(|i| eq_ci(&i.exchange, &q.exchange))
        .filter(|i| eq_ci(&i.instrument_type, &q.instrument_type))
        .filter(|i| eq_ci(&i.segment, &q.segment))
        .filter(|i| q.expiry.as_ref().is_none_or(|e| i.expiry == e.trim()))
        .filter(|i| q.strike.is_none_or(|s| (i.strike - s).abs() < 1e-6))
        .filter_map(|i| rank(i, &text).map(|r| (r, i)))
        .collect();

    hits.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then_with(|| (a.instrument_type != "EQ").cmp(&(b.instrument_type != "EQ")))
            .then_with(|| a.expiry.is_empty().cmp(&b.expiry.is_empty()).reverse())
            .then_with(|| a.expiry.cmp(&b.expiry))
            .then_with(|| a.strike.total_cmp(&b.strike))
            .then_with(|| a.tradingsymbol.cmp(&b.tradingsymbol))
    });
    hits.into_iter()
        .take(q.limit.max(1))
        .map(|(_, i)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruments::tests::sample_index;

    fn q(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            limit: 20,
            ..Default::default()
        }
    }

    #[test]
    fn exact_symbol_first() {
        let idx = sample_index();
        let r = search(&idx, &q("infy"));
        assert_eq!(r[0].tradingsymbol, "INFY");
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn filters_options_by_expiry_and_type() {
        let idx = sample_index();
        let r = search(
            &idx,
            &SearchQuery {
                text: "NIFTY".into(),
                instrument_type: Some("ce".into()),
                expiry: Some("2026-11-24".into()),
                limit: 20,
                ..Default::default()
            },
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tradingsymbol, "NIFTY26NOV24000CE");
    }

    #[test]
    fn derivatives_sorted_by_expiry() {
        let idx = sample_index();
        let r = search(
            &idx,
            &SearchQuery {
                text: "NIFTY".into(),
                exchange: Some("NFO".into()),
                limit: 20,
                ..Default::default()
            },
        );
        let expiries: Vec<_> = r.iter().map(|i| i.expiry.as_str()).collect();
        let mut sorted = expiries.clone();
        sorted.sort();
        assert_eq!(expiries, sorted);
    }

    #[test]
    fn respects_limit() {
        let idx = sample_index();
        assert_eq!(search(&idx, &SearchQuery { limit: 2, ..q("") }).len(), 2);
    }
}
