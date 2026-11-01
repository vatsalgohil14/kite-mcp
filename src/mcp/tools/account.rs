use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::KiteError;
use crate::mcp::server::{KiteMcp, ToolResult};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarginsArgs {
    /// `equity` or `commodity`. Omit for both.
    #[serde(default)]
    pub segment: Option<String>,
}

#[tool_router(router = account_router, vis = "pub(crate)")]
impl KiteMcp {
    /// Get funds and margins: available cash, collateral, opening balance, and margin
    /// utilised (SPAN, exposure, option premium, M2M). `net` is the usable balance.
    #[tool(annotations(
        title = "Get funds & margins",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_margins(&self, Parameters(args): Parameters<MarginsArgs>) -> ToolResult {
        let result = match args.segment.as_deref().map(str::to_ascii_lowercase) {
            None => self.client.margins().await.and_then(|m| {
                serde_json::to_value(m).map_err(|e| KiteError::Decode(e.to_string()))
            }),
            Some(seg) if seg == "equity" || seg == "commodity" => {
                self.client.segment_margins(&seg).await.and_then(|m| {
                    serde_json::to_value(m).map_err(|e| KiteError::Decode(e.to_string()))
                })
            }
            Some(other) => Err(KiteError::InvalidInput(format!(
                "segment must be `equity` or `commodity`, got `{other}`"
            ))),
        };
        self.respond(result)
    }
}
