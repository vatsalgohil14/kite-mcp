//! Kite login flow and session lifecycle.
//!
//! 1. `start_login` builds the Kite login URL and starts a one-shot callback server.
//! 2. Kite redirects to `http://127.0.0.1:<port>/callback?request_token=...`.
//! 3. The token is exchanged for an access token, which is saved to disk.

mod callback;
pub mod store;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::Serialize;

use crate::config::Config;
use crate::error::{KiteError, Result};
use crate::kite::KiteClient;
use store::StoredSession;

pub struct Auth {
    config: Config,
    client: Arc<KiteClient>,
    session: Mutex<Option<StoredSession>>,
    /// CSRF state of the login currently waiting for its callback.
    pending_state: Mutex<Option<String>>,
    /// Whether the login callback server is currently listening.
    callback_running: AtomicBool,
}

#[derive(Debug, Serialize)]
pub struct LoginStart {
    pub login_url: String,
    pub redirect_url: String,
    pub callback_listening: bool,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct SessionStatus {
    pub logged_in: bool,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub expires_at: Option<String>,
    pub login_in_progress: bool,
}

impl Auth {
    pub fn new(config: Config, client: Arc<KiteClient>) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            session: Mutex::new(None),
            pending_state: Mutex::new(None),
            callback_running: AtomicBool::new(false),
        })
    }

    /// Loads a still-valid session from disk, if any.
    pub fn restore(&self) -> Result<()> {
        self.sync();
        Ok(())
    }

    /// Picks up a session saved by another process (Chat, Code or `kite-mcp login`) and
    /// drops our own once it has expired. Called before every tool call; the file is tiny.
    pub fn sync(&self) {
        let now = Utc::now();
        let on_disk = match store::load(&self.config.session_path()) {
            Ok(s) => s.filter(|s| !s.is_expired(now)),
            Err(e) => {
                tracing::warn!("could not read saved session: {e}");
                None
            }
        };
        let mut session = self.session.lock().unwrap();
        match on_disk {
            Some(disk) if session.as_ref().map(|s| &s.access_token) != Some(&disk.access_token) => {
                tracing::info!("using Kite session for {} from disk", disk.user_id);
                self.client
                    .set_access_token(Some(disk.access_token.clone()));
                *session = Some(disk);
            }
            Some(_) => {}
            None => {
                if session.as_ref().is_some_and(|s| s.is_expired(now)) {
                    self.client.set_access_token(None);
                    *session = None;
                }
            }
        }
    }

    pub fn login_url(&self, state: &str) -> String {
        format!(
            "{}?v=3&api_key={}&redirect_params=state%3D{}",
            self.config.login_base, self.config.api_key, state
        )
    }

    pub async fn start_login(self: &Arc<Self>) -> Result<LoginStart> {
        self.config.require_credentials()?;
        let state = hex::encode(rand::random::<[u8; 16]>());
        *self.pending_state.lock().unwrap() = Some(state.clone());

        let (callback_listening, note) = match callback::spawn(self.clone()).await {
            Ok(()) => (
                true,
                "Open login_url in a browser and sign in. The session is captured automatically; \
                 call `session_status` afterwards to confirm."
                    .to_string(),
            ),
            Err(e) => (
                false,
                format!(
                    "Could not start the callback server ({e}). After signing in, copy the \
                     `request_token` from the redirected URL and call `complete_login`."
                ),
            ),
        };
        Ok(LoginStart {
            login_url: self.login_url(&state),
            redirect_url: self.config.redirect_url(),
            callback_listening,
            note,
        })
    }

    /// Validates the CSRF state from the callback. A missing state is rejected.
    pub(crate) fn check_state(&self, state: Option<&str>) -> bool {
        let pending = self.pending_state.lock().unwrap();
        matches!((pending.as_deref(), state), (Some(p), Some(s)) if p == s)
    }

    /// Exchanges `request_token` for an access token and persists the session.
    pub async fn complete_login(&self, request_token: &str) -> Result<SessionStatus> {
        self.config.require_credentials()?;
        let request_token = request_token.trim();
        if request_token.is_empty() {
            return Err(KiteError::Login("request_token is empty".into()));
        }
        let data = self
            .client
            .generate_session(request_token, &self.config.api_secret)
            .await?;
        let now = Utc::now();
        let session = StoredSession {
            user_id: data.user_id,
            user_name: data.user_name,
            access_token: data.access_token,
            created_at: now,
            expires_at: store::next_expiry(now),
        };
        store::save(&self.config.session_path(), &session)?;
        self.client
            .set_access_token(Some(session.access_token.clone()));
        *self.session.lock().unwrap() = Some(session);
        *self.pending_state.lock().unwrap() = None;
        tracing::info!("Kite login complete");
        Ok(self.status())
    }

    pub async fn logout(&self) -> Result<()> {
        if self.client.access_token().is_some()
            && let Err(e) = self.client.invalidate_session().await
        {
            tracing::warn!("invalidate_session failed (clearing locally anyway): {e}");
        }
        self.clear()
    }

    /// Drops our session, e.g. after a `TokenException`. The saved file is removed only if
    /// it still holds the same token, so a newer login by another process survives.
    pub fn clear(&self) -> Result<()> {
        let mine = self.client.access_token();
        self.client.set_access_token(None);
        *self.session.lock().unwrap() = None;
        let path = self.config.session_path();
        let same = store::load(&path)
            .ok()
            .flatten()
            .is_some_and(|s| Some(&s.access_token) == mine.as_ref());
        if same {
            store::remove(&path)?;
        }
        Ok(())
    }

    pub fn status(&self) -> SessionStatus {
        let session = self.session.lock().unwrap();
        let valid = session.as_ref().filter(|s| !s.is_expired(Utc::now()));
        SessionStatus {
            logged_in: valid.is_some(),
            user_id: valid.map(|s| s.user_id.clone()),
            user_name: valid.map(|s| s.user_name.clone()),
            expires_at: valid.map(|s| s.expires_at.to_rfc3339()),
            login_in_progress: self.pending_state.lock().unwrap().is_some(),
        }
    }

    pub fn callback_port(&self) -> u16 {
        self.config.callback_port
    }
}
