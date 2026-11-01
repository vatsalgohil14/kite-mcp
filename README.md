# personal-kite-mcp

Personal MCP server for Zerodha Kite (Kite Connect v3), written in Rust. Claude launches it
locally over stdio. See [PLAN.md](PLAN.md) for the full roadmap.

## Status

Phase 1 — foundation: login flow, session persistence, profile.

| Tool | What it does |
|---|---|
| `login` | Returns a Kite login URL and captures the session via a local callback |
| `complete_login` | Manual fallback: pass the `request_token` from the redirect URL |
| `session_status` | Logged-in user and session expiry |
| `logout` | Invalidates the token and deletes the saved session |
| `get_profile` | User profile, enabled exchanges/products/order types |

## Setup

1. Create a Kite Connect app at <https://developers.kite.trade> and set its
   **Redirect URL** to `http://127.0.0.1:5005/callback`.
2. Put your credentials in `~/.config/personal-kite-mcp/config.toml`
   (see [config.example.toml](config.example.toml)):

   ```bash
   mkdir -p ~/.config/personal-kite-mcp
   cp config.example.toml ~/.config/personal-kite-mcp/config.toml
   chmod 600 ~/.config/personal-kite-mcp/config.toml
   ```

   Or set `KITE_API_KEY` / `KITE_API_SECRET` in the environment.
3. Build: `cargo build --release`
4. Register with Claude Code:

   ```bash
   claude mcp add kite -- "$PWD/target/release/personal-kite-mcp"
   ```

   Or Claude Desktop (`~/Library/Application Support/Claude/claude_desktop_config.json`):

   ```json
   {
     "mcpServers": {
       "kite": { "command": "/Users/<you>/Desktop/personal-kite-mcp/target/release/personal-kite-mcp" }
     }
   }
   ```

5. Ask Claude to "log in to Kite", open the link, sign in. The session is saved to
   `~/.config/personal-kite-mcp/session.json` (mode 600) and reused until it expires (~06:00 IST).

## Configuration

| Key / env var | Default |
|---|---|
| `api_key` / `KITE_API_KEY` | — |
| `api_secret` / `KITE_API_SECRET` | — |
| `callback_port` / `KITE_CALLBACK_PORT` | `5005` |
| config dir / `KITE_MCP_HOME` | `~/.config/personal-kite-mcp` |
| log level / `KITE_MCP_LOG` | `info` (logs go to stderr) |

## Development

```bash
cargo test
cargo fmt && cargo clippy --all-targets -- -D warnings
```

Integration tests run against a mocked Kite API (`wiremock`); no account needed.
