use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A Kite session persisted to disk so restarts don't require a new login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub user_id: String,
    pub user_name: String,
    pub access_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl StoredSession {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }
}

fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset")
}

/// Kite access tokens are flushed daily around 06:00 IST. Returns the first 06:00 IST after `t`.
pub fn next_expiry(t: DateTime<Utc>) -> DateTime<Utc> {
    let local = t.with_timezone(&ist());
    let six = NaiveTime::from_hms_opt(6, 0, 0).expect("valid time");
    let mut day = local.date_naive();
    if local.time() >= six {
        day += Duration::days(1);
    }
    ist()
        .from_local_datetime(&day.and_time(six))
        .single()
        .expect("fixed offset is unambiguous")
        .with_timezone(&Utc)
}

pub fn load(path: &Path) -> Result<Option<StoredSession>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    match serde_json::from_str(&text) {
        Ok(s) => Ok(Some(s)),
        Err(e) => {
            tracing::warn!("ignoring unreadable session file {}: {e}", path.display());
            Ok(None)
        }
    }
}

pub fn save(path: &Path, session: &StoredSession) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(session).expect("session serializes");
    std::fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn expiry_is_next_six_am_ist() {
        // 10:00 IST -> next day 06:00 IST (00:30 UTC)
        assert_eq!(
            next_expiry(utc("2026-11-02T04:30:00Z")),
            utc("2026-11-03T00:30:00Z")
        );
        // 03:00 IST -> same day 06:00 IST
        assert_eq!(
            next_expiry(utc("2026-11-01T21:30:00Z")),
            utc("2026-11-02T00:30:00Z")
        );
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let now = Utc::now();
        let s = StoredSession {
            user_id: "AB1234".into(),
            user_name: "Test".into(),
            access_token: "tok".into(),
            created_at: now,
            expires_at: next_expiry(now),
        };
        save(&path, &s).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.access_token, "tok");
        remove(&path).unwrap();
        assert!(load(&path).unwrap().is_none());
    }
}
