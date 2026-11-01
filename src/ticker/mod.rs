//! Live market data over Kite's WebSocket. One background connection is opened on the
//! first subscription, kept alive with reconnects, and closed when nothing is subscribed.
//! The latest tick per instrument and recent order updates are cached for the tools.

pub mod parser;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::error::{KiteError, Result};
use crate::kite::KiteClient;
use parser::Tick;

/// Kite allows up to 3000 instruments per connection.
pub const MAX_SUBSCRIPTIONS: usize = 3000;
const ORDER_UPDATES_KEPT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ltp,
    Quote,
    Full,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ltp" => Ok(Mode::Ltp),
            "quote" => Ok(Mode::Quote),
            "full" => Ok(Mode::Full),
            other => Err(KiteError::InvalidInput(format!(
                "mode must be ltp, quote or full; got `{other}`"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Mode::Ltp => "ltp",
            Mode::Quote => "quote",
            Mode::Full => "full",
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub connected: bool,
    pub subscriptions: usize,
    pub cached_ticks: usize,
    pub reconnects: u32,
    pub last_message_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct State {
    subs: HashMap<u32, Mode>,
    ticks: HashMap<u32, (Tick, String)>,
    order_updates: VecDeque<Value>,
    status: Status,
}

enum Cmd {
    Subscribe(Vec<u32>, Mode),
    Unsubscribe(Vec<u32>),
}

pub struct Ticker {
    url: String,
    client: Arc<KiteClient>,
    state: Arc<Mutex<State>>,
    cmd: Mutex<Option<mpsc::UnboundedSender<Cmd>>>,
}

impl Ticker {
    pub fn new(url: impl Into<String>, client: Arc<KiteClient>) -> Self {
        Self {
            url: url.into(),
            client,
            state: Arc::default(),
            cmd: Mutex::new(None),
        }
    }

    pub fn subscribe(&self, tokens: &[u32], mode: Mode) -> Result<()> {
        let token = self.client.access_token().ok_or(KiteError::NotLoggedIn)?;
        {
            let mut st = self.state.lock().unwrap();
            let new = tokens.iter().filter(|t| !st.subs.contains_key(t)).count();
            if st.subs.len() + new > MAX_SUBSCRIPTIONS {
                return Err(KiteError::InvalidInput(format!(
                    "at most {MAX_SUBSCRIPTIONS} subscriptions per connection"
                )));
            }
            for t in tokens {
                st.subs.insert(*t, mode);
            }
            st.status.subscriptions = st.subs.len();
        }
        let mut cmd = self.cmd.lock().unwrap();
        let alive = cmd.as_ref().is_some_and(|tx| !tx.is_closed());
        if alive {
            let _ = cmd
                .as_ref()
                .expect("checked")
                .send(Cmd::Subscribe(tokens.to_vec(), mode));
        } else {
            // A fresh worker subscribes to everything in `subs` on connect.
            let (tx, rx) = mpsc::unbounded_channel();
            *cmd = Some(tx);
            // The request needs a path, so `wss://host` becomes `wss://host/?...`.
            let url = format!(
                "{}/?api_key={}&access_token={}",
                self.url.trim_end_matches('/'),
                self.client.api_key(),
                token
            );
            tokio::spawn(run(url, self.state.clone(), rx));
        }
        Ok(())
    }

    /// Unsubscribes the given tokens, or everything when `tokens` is empty.
    pub fn unsubscribe(&self, tokens: &[u32]) -> usize {
        let removed: Vec<u32> = {
            let mut st = self.state.lock().unwrap();
            let removed: Vec<u32> = if tokens.is_empty() {
                st.subs.keys().copied().collect()
            } else {
                tokens
                    .iter()
                    .copied()
                    .filter(|t| st.subs.contains_key(t))
                    .collect()
            };
            for t in &removed {
                st.subs.remove(t);
                st.ticks.remove(t);
            }
            st.status.subscriptions = st.subs.len();
            removed
        };
        if let Some(tx) = self.cmd.lock().unwrap().as_ref() {
            let _ = tx.send(Cmd::Unsubscribe(removed.clone()));
        }
        removed.len()
    }

    /// Latest ticks with their receive time, for the given tokens (or all).
    pub fn ticks(&self, tokens: &[u32]) -> Vec<(Tick, String)> {
        let st = self.state.lock().unwrap();
        if tokens.is_empty() {
            st.ticks.values().cloned().collect()
        } else {
            tokens
                .iter()
                .filter_map(|t| st.ticks.get(t).cloned())
                .collect()
        }
    }

    pub fn subscriptions(&self) -> Vec<(u32, Mode)> {
        let st = self.state.lock().unwrap();
        st.subs.iter().map(|(t, m)| (*t, *m)).collect()
    }

    pub fn order_updates(&self, limit: usize) -> Vec<Value> {
        let st = self.state.lock().unwrap();
        st.order_updates.iter().rev().take(limit).cloned().collect()
    }

    pub fn status(&self) -> Status {
        let st = self.state.lock().unwrap();
        let mut s = st.status.clone();
        s.cached_ticks = st.ticks.len();
        s
    }
}

fn mode_messages(subs: &[(u32, Mode)]) -> Vec<String> {
    let mut by_mode: HashMap<&'static str, Vec<u32>> = HashMap::new();
    for (t, m) in subs {
        by_mode.entry(m.as_str()).or_default().push(*t);
    }
    let all: Vec<u32> = subs.iter().map(|(t, _)| *t).collect();
    let mut out = vec![json!({ "a": "subscribe", "v": all }).to_string()];
    for (mode, tokens) in by_mode {
        out.push(json!({ "a": "mode", "v": [mode, tokens] }).to_string());
    }
    out
}

fn handle_text(state: &Mutex<State>, text: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let mut st = state.lock().unwrap();
    match msg["type"].as_str() {
        Some("order") => {
            st.order_updates.push_back(json!({
                "received_at": Utc::now().to_rfc3339(),
                "data": msg["data"],
            }));
            while st.order_updates.len() > ORDER_UPDATES_KEPT {
                st.order_updates.pop_front();
            }
        }
        Some("error") => st.status.last_error = msg["data"].as_str().map(String::from),
        _ => {}
    }
}

/// Connection loop: connect, (re)subscribe, read until disconnect, back off, repeat.
/// Exits when there are no subscriptions left or the command channel closes.
async fn run(url: String, state: Arc<Mutex<State>>, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut backoff = Duration::from_secs(1);
    state.lock().unwrap().status.running = true;
    loop {
        let subs: Vec<(u32, Mode)> = {
            let st = state.lock().unwrap();
            st.subs.iter().map(|(t, m)| (*t, *m)).collect()
        };
        if subs.is_empty() {
            break;
        }

        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((mut ws, _)) => {
                backoff = Duration::from_secs(1);
                {
                    let mut st = state.lock().unwrap();
                    st.status.connected = true;
                    st.status.last_error = None;
                }
                tracing::info!("ticker connected, subscribing {} instruments", subs.len());
                let mut ok = true;
                for m in mode_messages(&subs) {
                    if ws.send(Message::Text(m.into())).await.is_err() {
                        ok = false;
                        break;
                    }
                }
                while ok {
                    tokio::select! {
                        msg = ws.next() => match msg {
                            Some(Ok(Message::Binary(b))) => {
                                let ticks = parser::parse_frame(&b);
                                let now = Utc::now().to_rfc3339();
                                let mut st = state.lock().unwrap();
                                st.status.last_message_at = Some(now.clone());
                                for t in ticks {
                                    if st.subs.contains_key(&t.instrument_token) {
                                        st.ticks.insert(t.instrument_token, (t, now.clone()));
                                    }
                                }
                            }
                            Some(Ok(Message::Text(t))) => handle_text(&state, t.as_str()),
                            Some(Ok(Message::Close(frame))) => {
                                state.lock().unwrap().status.last_error =
                                    Some(format!("closed by server: {frame:?}"));
                                ok = false;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                state.lock().unwrap().status.last_error = Some(e.to_string());
                                ok = false;
                            }
                            None => ok = false,
                        },
                        cmd = rx.recv() => match cmd {
                            Some(Cmd::Subscribe(tokens, mode)) => {
                                for m in mode_messages(&tokens.iter().map(|t| (*t, mode)).collect::<Vec<_>>()) {
                                    if ws.send(Message::Text(m.into())).await.is_err() { ok = false; }
                                }
                            }
                            Some(Cmd::Unsubscribe(tokens)) => {
                                if !tokens.is_empty() {
                                    let m = json!({ "a": "unsubscribe", "v": tokens }).to_string();
                                    if ws.send(Message::Text(m.into())).await.is_err() { ok = false; }
                                }
                                if state.lock().unwrap().subs.is_empty() {
                                    let _ = ws.close(None).await;
                                    ok = false;
                                }
                            }
                            None => {
                                let _ = ws.close(None).await;
                                state.lock().unwrap().status = Status::default();
                                return;
                            }
                        },
                    }
                }
            }
            Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                if matches!(resp.status().as_u16(), 401 | 403) =>
            {
                // Not transient: no market-data subscription, or the token is invalid.
                tracing::warn!("ticker rejected with HTTP {}", resp.status());
                let mut st = state.lock().unwrap();
                st.status.last_error = Some(format!(
                    "HTTP {}: Kite refused the live data connection. Live ticks need the paid \
                     Kite Connect plan, and a valid session (call `login` if it expired). \
                     Subscriptions were cleared.",
                    resp.status().as_u16()
                ));
                st.subs.clear();
                st.status.subscriptions = 0;
                break;
            }
            Err(e) => {
                tracing::warn!("ticker connect failed: {e}");
                state.lock().unwrap().status.last_error = Some(e.to_string());
            }
        }

        {
            let mut st = state.lock().unwrap();
            st.status.connected = false;
            if st.subs.is_empty() {
                break;
            }
            st.status.reconnects += 1;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
    let mut st = state.lock().unwrap();
    st.status.running = false;
    st.status.connected = false;
    tracing::info!("ticker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_messages_group_by_mode() {
        let msgs = mode_messages(&[(1, Mode::Ltp), (2, Mode::Full), (3, Mode::Full)]);
        let first: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(first["a"], "subscribe");
        assert_eq!(first["v"].as_array().unwrap().len(), 3);
        assert_eq!(msgs.len(), 3);
        assert!(
            msgs.iter()
                .any(|m| m.contains("\"full\"") && m.contains('2') && m.contains('3'))
        );
    }

    #[test]
    fn order_updates_are_kept_and_capped() {
        let state = Mutex::new(State::default());
        for i in 0..(ORDER_UPDATES_KEPT + 5) {
            handle_text(
                &state,
                &json!({ "type": "order", "data": { "order_id": i } }).to_string(),
            );
        }
        handle_text(&state, r#"{"type":"error","data":"bad token"}"#);
        let st = state.lock().unwrap();
        assert_eq!(st.order_updates.len(), ORDER_UPDATES_KEPT);
        assert_eq!(st.status.last_error.as_deref(), Some("bad token"));
    }
}
