use std::sync::Arc;

use anyhow::Context;
use kite_mcp::auth::Auth;
use kite_mcp::config::Config;
use kite_mcp::instruments::InstrumentStore;
use kite_mcp::kite::KiteClient;
use kite_mcp::mcp::{KiteMcp, Services};
use kite_mcp::safety::Safety;
use kite_mcp::ticker::Ticker;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // stdout carries the MCP protocol, so logs must go to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("KITE_MCP_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let config = Config::load().context("loading config")?;
    if let Err(e) = config.require_credentials() {
        // Keep running so `session_status` etc. still work and report the problem.
        tracing::warn!("{e}");
    }

    let client = Arc::new(KiteClient::new(&config.api_base, &config.api_key)?);
    let instruments = Arc::new(InstrumentStore::new(client.clone(), config.home.clone()));
    let safety = Arc::new(Safety::new(config.safety.clone(), config.home.clone()));
    let ticker = Arc::new(Ticker::new(config.ticker_url.clone(), client.clone()));
    let auth = Auth::new(config, client.clone());
    auth.restore().context("restoring saved session")?;

    if std::env::args().nth(1).as_deref() == Some("login") {
        return login_cli(&auth).await;
    }

    tracing::info!("starting {} on stdio", env!("CARGO_PKG_NAME"));
    let service = KiteMcp::new(Services {
        client,
        auth,
        instruments,
        safety,
        ticker,
    })
    .serve(stdio())
    .await
    .context("starting MCP server")?;
    service.waiting().await?;
    Ok(())
}

/// `kite-mcp login`: runs the login flow from a terminal and saves the session, so the
/// MCP server starts already logged in.
async fn login_cli(auth: &Arc<Auth>) -> anyhow::Result<()> {
    let start = auth.start_login().await?;
    if !start.callback_listening {
        anyhow::bail!("{}", start.note);
    }
    println!(
        "Open this URL and sign in to Kite:\n\n  {}\n",
        start.login_url
    );
    println!(
        "Waiting for the redirect to {} (5 minutes)...",
        start.redirect_url
    );
    for _ in 0..300 {
        let status = auth.status();
        if status.logged_in {
            println!(
                "Logged in as {} ({}). Session valid until {}.",
                status.user_name.unwrap_or_default(),
                status.user_id.unwrap_or_default(),
                status.expires_at.unwrap_or_default()
            );
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    anyhow::bail!("timed out waiting for login")
}
