use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{FixedOffset, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Error,
    DryRun,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    ts: String,
    /// IST date, used for daily counts.
    date_ist: String,
    action: String,
    outcome: Outcome,
    request: serde_json::Value,
    response: serde_json::Value,
}

/// Append-only JSONL log of every mutating call.
pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

fn today_ist() -> String {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset");
    Utc::now()
        .with_timezone(&ist)
        .format("%Y-%m-%d")
        .to_string()
}

impl AuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn record(
        &self,
        action: &str,
        outcome: Outcome,
        request: &serde_json::Value,
        response: &serde_json::Value,
    ) {
        let entry = Entry {
            ts: Utc::now().to_rfc3339(),
            date_ist: today_ist(),
            action: action.into(),
            outcome,
            request: request.clone(),
            response: response.clone(),
        };
        let _g = self.lock.lock().unwrap();
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            writeln!(f, "{}", serde_json::to_string(&entry).unwrap_or_default())
        })();
        if let Err(e) = result {
            tracing::error!("failed to write audit log {}: {e}", self.path.display());
        }
    }

    /// Orders successfully placed today (IST), including GTT placements.
    pub fn orders_placed_today(&self) -> usize {
        let _g = self.lock.lock().unwrap();
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return 0;
        };
        let today = today_ist();
        text.lines()
            .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
            .filter(|e| e.date_ist == today && e.outcome == Outcome::Ok)
            .filter(|e| matches!(e.action.as_str(), "place_order" | "place_gtt"))
            .count()
    }

    /// The most recent `n` entries, newest first.
    pub fn recent(&self, n: usize) -> Vec<serde_json::Value> {
        let _g = self.lock.lock().unwrap();
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        text.lines()
            .rev()
            .take(n)
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_only_successful_placements_today() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        log.record("place_order", Outcome::Ok, &json!({}), &json!({}));
        log.record("place_order", Outcome::Error, &json!({}), &json!({}));
        log.record("place_order", Outcome::DryRun, &json!({}), &json!({}));
        log.record("cancel_order", Outcome::Ok, &json!({}), &json!({}));
        log.record("place_gtt", Outcome::Ok, &json!({}), &json!({}));
        assert_eq!(log.orders_placed_today(), 2);
        assert_eq!(log.recent(2).len(), 2);
        assert_eq!(log.recent(1)[0]["action"], "place_gtt");
    }
}
