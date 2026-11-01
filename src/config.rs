use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{KiteError, Result};

pub const DEFAULT_API_BASE: &str = "https://api.kite.trade";
pub const DEFAULT_LOGIN_BASE: &str = "https://kite.zerodha.com/connect/login";
pub const DEFAULT_CALLBACK_PORT: u16 = 5005;

/// Runtime configuration, loaded from `<home>/config.toml` and overridden by env vars.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub api_key: String,
    pub api_secret: String,
    /// Port of the local login callback server. The Kite app's redirect URL must be
    /// `http://127.0.0.1:<port>/callback`.
    pub callback_port: u16,
    pub api_base: String,
    pub login_base: String,
    pub ticker_url: String,
    pub safety: SafetyConfig,
    /// Directory holding config, session and logs. Not read from the file itself.
    #[serde(skip)]
    pub home: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            api_secret: String::new(),
            callback_port: DEFAULT_CALLBACK_PORT,
            api_base: DEFAULT_API_BASE.into(),
            login_base: DEFAULT_LOGIN_BASE.into(),
            ticker_url: DEFAULT_TICKER_URL.into(),
            safety: SafetyConfig::default(),
            home: PathBuf::new(),
        }
    }
}

pub const DEFAULT_TICKER_URL: &str = "wss://ws.kite.trade";

/// Guards for tools that place, modify or cancel orders (`[safety]` in config.toml).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    /// Don't register any tool that changes orders, positions, GTTs or alerts.
    pub read_only: bool,
    /// Mutating tools return a preview and a `confirm_token`; nothing is sent until
    /// `confirm_action` is called with it.
    pub require_confirmation: bool,
    /// Validate and log orders but never send them to Kite.
    pub dry_run: bool,
    /// If non-empty, only these exchanges may be traded (e.g. ["NSE", "BSE"]).
    pub allowed_exchanges: Vec<String>,
    /// If non-empty, only these products may be used (e.g. ["CNC"]).
    pub allowed_products: Vec<String>,
    /// Tradingsymbols that may never be traded.
    pub blocked_symbols: Vec<String>,
    /// Tag attached to every order (alphanumeric, max 20 chars).
    pub order_tag: String,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            read_only: false,
            require_confirmation: true,
            dry_run: false,
            allowed_exchanges: Vec::new(),
            allowed_products: Vec::new(),
            blocked_symbols: Vec::new(),
            order_tag: "kitemcp".into(),
        }
    }
}

impl Config {
    /// `$KITE_MCP_HOME`, else `~/.config/kite-mcp`.
    pub fn default_home() -> PathBuf {
        if let Ok(dir) = std::env::var("KITE_MCP_HOME") {
            return PathBuf::from(dir);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".config").join("kite-mcp")
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_home())
    }

    pub fn load_from(home: &Path) -> Result<Self> {
        let file = home.join("config.toml");
        let mut cfg: Config = if file.exists() {
            let text = std::fs::read_to_string(&file)?;
            toml::from_str(&text)
                .map_err(|e| KiteError::Config(format!("{}: {e}", file.display())))?
        } else {
            Config::default()
        };
        cfg.home = home.to_path_buf();

        if let Ok(v) = std::env::var("KITE_API_KEY") {
            cfg.api_key = v;
        }
        if let Ok(v) = std::env::var("KITE_API_SECRET") {
            cfg.api_secret = v;
        }
        if let Some(port) = std::env::var("KITE_CALLBACK_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            cfg.callback_port = port;
        }
        Ok(cfg)
    }

    /// Fails with a setup hint if credentials are missing.
    pub fn require_credentials(&self) -> Result<()> {
        if self.api_key.is_empty() || self.api_secret.is_empty() {
            return Err(KiteError::Config(format!(
                "api_key / api_secret not set. Put them in {} or set KITE_API_KEY and KITE_API_SECRET.",
                self.home.join("config.toml").display()
            )));
        }
        Ok(())
    }

    pub fn session_path(&self) -> PathBuf {
        self.home.join("session.json")
    }

    pub fn redirect_url(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.callback_port)
    }
}
