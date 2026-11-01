use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a preview stays confirmable.
pub const CONFIRM_TTL: Duration = Duration::from_secs(120);

/// A mutating call that was previewed and is waiting for `confirm_action`.
#[derive(Debug, Clone)]
pub struct PendingAction {
    /// Tool name, e.g. `place_order`.
    pub action: String,
    /// The validated request, replayed on confirmation.
    pub request: serde_json::Value,
    pub summary: String,
}

#[derive(Default)]
pub struct Confirmations {
    pending: Mutex<HashMap<String, (Instant, PendingAction)>>,
}

impl Confirmations {
    /// Stores an action and returns its one-time token.
    pub fn add(&self, action: PendingAction) -> String {
        let token = hex::encode(rand::random::<[u8; 6]>());
        let mut map = self.pending.lock().unwrap();
        map.retain(|_, (t, _)| t.elapsed() < CONFIRM_TTL);
        map.insert(token.clone(), (Instant::now(), action));
        token
    }

    /// Removes and returns the action if the token exists and has not expired.
    pub fn take(&self, token: &str) -> Option<PendingAction> {
        let mut map = self.pending.lock().unwrap();
        let (created, action) = map.remove(token.trim())?;
        (created.elapsed() < CONFIRM_TTL).then_some(action)
    }

    pub fn list(&self) -> Vec<(String, PendingAction, u64)> {
        let mut map = self.pending.lock().unwrap();
        map.retain(|_, (t, _)| t.elapsed() < CONFIRM_TTL);
        map.iter()
            .map(|(k, (t, a))| {
                let left = CONFIRM_TTL.saturating_sub(t.elapsed()).as_secs();
                (k.clone(), a.clone(), left)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_single_use() {
        let c = Confirmations::default();
        let t = c.add(PendingAction {
            action: "place_order".into(),
            request: serde_json::json!({}),
            summary: "BUY 1 INFY".into(),
        });
        assert_eq!(c.list().len(), 1);
        assert!(c.take(&t).is_some());
        assert!(c.take(&t).is_none());
        assert!(c.take("nope").is_none());
    }
}
