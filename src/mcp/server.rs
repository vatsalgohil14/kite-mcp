use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::auth::Auth;
use crate::error::KiteError;
use crate::instruments::InstrumentStore;
use crate::kite::KiteClient;
use crate::mcp::tools::mf::MfCache;
use crate::safety::Safety;
use crate::ticker::Ticker;

const INSTRUCTIONS: &str = "Tools for the user's Zerodha Kite trading account via Kite Connect. \
Kite sessions expire daily (~06:00 IST). If a tool reports that the user is not logged in, \
call `login`, give the user the login_url to open, then call `session_status` to confirm. \
Instruments are written as EXCHANGE:TRADINGSYMBOL (e.g. NSE:INFY, NFO:NIFTY26NOV24000CE); \
use `search_instruments` to find exact symbols for futures and options. \
All prices are in INR and all times are IST. \
Tools that place, modify or cancel orders, convert positions, or create GTTs / order-placing \
alerts involve real money. They return a preview with a `confirm_token`: always show the \
summary to the user and call `confirm_action` only after the user explicitly approves. \
Never confirm on your own initiative.";

#[derive(Clone)]
pub struct KiteMcp {
    pub(crate) client: Arc<KiteClient>,
    pub(crate) auth: Arc<Auth>,
    pub(crate) instruments: Arc<InstrumentStore>,
    pub(crate) safety: Arc<Safety>,
    pub(crate) ticker: Arc<Ticker>,
    pub(crate) mf_cache: Arc<MfCache>,
    tool_router: ToolRouter<Self>,
}

/// Shared services the server is built from.
pub struct Services {
    pub client: Arc<KiteClient>,
    pub auth: Arc<Auth>,
    pub instruments: Arc<InstrumentStore>,
    pub safety: Arc<Safety>,
    pub ticker: Arc<Ticker>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompleteLoginArgs {
    /// The `request_token` query parameter from the URL Kite redirected to after login.
    pub request_token: String,
}

pub(crate) type ToolResult = Result<CallToolResult, ErrorData>;

impl KiteMcp {
    pub fn new(s: Services) -> Self {
        let mut router = Self::session_router()
            + Self::account_router()
            + Self::portfolio_router()
            + Self::orders_router()
            + Self::market_router()
            + Self::margins_router()
            + Self::gtt_read_router()
            + Self::alerts_read_router()
            + Self::mf_router()
            + Self::live_router()
            + Self::insights_router()
            + Self::safety_router();
        // In read-only mode, tools that change anything are not registered at all.
        if !s.safety.config.read_only {
            router = router
                + Self::trading_router()
                + Self::gtt_write_router()
                + Self::alerts_write_router();
        }
        Self {
            client: s.client,
            auth: s.auth,
            instruments: s.instruments,
            safety: s.safety,
            ticker: s.ticker,
            mf_cache: Arc::default(),
            tool_router: router,
        }
    }

    /// Turns a Kite call result into a tool result. Kite errors are returned as tool
    /// errors (visible to the model) rather than protocol errors.
    pub(crate) fn respond<T: Serialize>(&self, result: Result<T, KiteError>) -> ToolResult {
        match result {
            Ok(value) => {
                let json = serde_json::to_string(&value)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            Err(e) => {
                let mut msg = e.to_string();
                if e.is_token_error() {
                    if let Err(clear_err) = self.auth.clear() {
                        tracing::warn!("failed to clear session: {clear_err}");
                    }
                    if !matches!(e, KiteError::NotLoggedIn) {
                        msg.push_str(" The Kite session has expired; call `login` again.");
                    }
                }
                if e.is_permission_error() {
                    msg.push_str(
                        " Market data (quotes, LTP, OHLC, historical candles) needs a paid \
                         Kite Connect plan; the free Personal app only covers account, \
                         portfolio and order data.",
                    );
                }
                Ok(CallToolResult::error(vec![ContentBlock::text(msg)]))
            }
        }
    }
}

#[tool_router(router = session_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Start the Kite login flow. Returns a login URL for the user to open in a browser;
    /// the session is captured automatically by a local callback server.
    #[tool(annotations(
        title = "Log in to Kite",
        read_only_hint = false,
        open_world_hint = true
    ))]
    async fn login(&self) -> ToolResult {
        let result = self.auth.start_login().await;
        self.respond(result)
    }

    /// Finish login manually using the `request_token` from Kite's redirect URL.
    /// Only needed if the automatic callback did not work.
    #[tool(annotations(
        title = "Complete Kite login",
        read_only_hint = false,
        open_world_hint = true
    ))]
    async fn complete_login(&self, Parameters(args): Parameters<CompleteLoginArgs>) -> ToolResult {
        let result = self.auth.complete_login(&args.request_token).await;
        self.respond(result)
    }

    /// Log out: invalidates the Kite access token and deletes the saved session.
    #[tool(annotations(
        title = "Log out of Kite",
        destructive_hint = false,
        open_world_hint = true
    ))]
    async fn logout(&self) -> ToolResult {
        let result = self.auth.logout().await.map(|()| "Logged out.");
        self.respond(result)
    }

    /// Show whether a Kite session is active, for which user, and when it expires.
    #[tool(annotations(title = "Kite session status", read_only_hint = true))]
    async fn session_status(&self) -> ToolResult {
        self.respond(Ok(self.auth.status()))
    }

    /// Get the logged-in user's Kite profile: user id, name, email, enabled exchanges,
    /// products and order types.
    #[tool(annotations(title = "Get profile", read_only_hint = true, open_world_hint = true))]
    async fn get_profile(&self) -> ToolResult {
        let result = self.client.profile().await;
        self.respond(result)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KiteMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}
