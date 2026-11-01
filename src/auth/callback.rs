//! One-shot local HTTP server that receives Kite's login redirect.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use tokio::sync::mpsc;

use super::Auth;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct Ctx {
    auth: Arc<Auth>,
    done: mpsc::Sender<()>,
}

/// Starts the callback server in the background. Returns once it is listening.
/// If one is already running (a previous `login` call), it is reused.
pub async fn spawn(auth: Arc<Auth>) -> std::io::Result<()> {
    if auth.callback_running.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], auth.callback_port()));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            auth.callback_running.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };
    let server_auth = auth.clone();

    let (tx, mut rx) = mpsc::channel::<()>(1);
    let app = Router::new()
        .route("/callback", get(handle))
        .with_state(Ctx { auth, done: tx });

    tokio::spawn(async move {
        let shutdown = async move {
            let _ = tokio::time::timeout(LOGIN_TIMEOUT, rx.recv()).await;
        };
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
        {
            tracing::error!("callback server error: {e}");
        }
        server_auth.callback_running.store(false, Ordering::SeqCst);
        tracing::info!("login callback server stopped");
    });
    tracing::info!("login callback server listening on {addr}");
    Ok(())
}

async fn handle(State(ctx): State<Ctx>, Query(q): Query<HashMap<String, String>>) -> Html<String> {
    if q.get("status").map(String::as_str) != Some("success") {
        return page(
            "Login was not successful",
            "Kite did not report a successful login.",
        );
    }
    if !ctx.auth.check_state(q.get("state").map(String::as_str)) {
        return page(
            "Login rejected",
            "The login state did not match. Call `login` again and use the new link.",
        );
    }
    let Some(token) = q.get("request_token") else {
        return page("Login failed", "No request_token in the redirect.");
    };

    let result = ctx.auth.complete_login(token).await;
    let _ = ctx.done.try_send(());
    match result {
        Ok(s) => page(
            "Logged in to Kite",
            &format!(
                "Signed in as {} ({}). You can close this tab and return to Claude.",
                s.user_name.unwrap_or_default(),
                s.user_id.unwrap_or_default()
            ),
        ),
        Err(e) => page("Login failed", &e.to_string()),
    }
}

fn page(title: &str, body: &str) -> Html<String> {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    Html(format!(
        "<!doctype html><meta charset=utf-8><title>{t}</title>\
         <body style=\"font-family:system-ui;max-width:32rem;margin:4rem auto\">\
         <h2>{t}</h2><p>{b}</p></body>",
        t = esc(title),
        b = esc(body)
    ))
}
