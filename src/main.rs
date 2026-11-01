use std::sync::Arc;

use anyhow::Context;
use kite_mcp::auth::Auth;
use kite_mcp::config::Config;
use kite_mcp::instruments::InstrumentStore;
use kite_mcp::kite::KiteClient;
use kite_mcp::mcp::KiteMcp;
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
    let auth = Auth::new(config, client.clone());
    auth.restore().context("restoring saved session")?;

    tracing::info!("starting {} on stdio", env!("CARGO_PKG_NAME"));
    let service = KiteMcp::new(client, auth, instruments)
        .serve(stdio())
        .await
        .context("starting MCP server")?;
    service.waiting().await?;
    Ok(())
}
