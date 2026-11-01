# kite-mcp

MCP server for Zerodha Kite (Kite Connect v3), written in Rust. Claude (Desktop chat or
Claude Code) launches it locally over stdio. See [PLAN.md](PLAN.md) for the design.

It covers the full Kite Connect v3 API: session, account, portfolio, orders, GTT, alerts,
margin and charge calculators, market data, historical candles, mutual funds and the live
WebSocket ticker — 55 tools in total.

## Tools

🟢 read-only · 🔴 changes your account (goes through the [safety layer](#safety))

| Area | Tool | What it does |
|---|---|---|
| Session | 🟢 `login` / `complete_login` | Kite login URL; session captured by a local callback (or paste the `request_token`) |
| | 🟢 `session_status` / `logout` | Logged-in user and expiry / invalidate the token |
| Account | 🟢 `get_profile` | Profile, enabled exchanges, products, order types |
| | 🟢 `get_margins` | Funds and margins (equity / commodity) |
| Portfolio | 🟢 `get_holdings` | Holdings with per-stock and total P&L, day change; filter/sort |
| | 🟢 `get_positions` | Net or day positions with P&L / M2M |
| | 🟢 `get_auctions` | Holdings eligible for auction |
| | 🟢 `portfolio_summary` | Holdings + positions + funds + mutual funds in one call, top movers |
| | 🔴 `convert_position` | Change a position's product (MIS ↔ CNC/NRML) |
| | 🔴 `authorise_holdings` | Start CDSL/TPIN authorisation to sell holdings |
| Orders | 🟢 `get_orders` | Today's order book; filter by status/symbol/tag |
| | 🟢 `get_order_history` | Every status transition of one order |
| | 🟢 `get_trades` / `get_order_trades` | Executed fills |
| | 🔴 `place_order` | regular / AMO / CO / iceberg / auction; MARKET / LIMIT / SL / SL-M; CNC / MIS / NRML / MTF; DAY / IOC / TTL |
| | 🔴 `modify_order` / `cancel_order` | Change or cancel an open order |
| Safety | 🔴 `confirm_action` | Execute a previewed action after the user approves |
| | 🟢 `list_pending_actions` | Previews waiting for confirmation |
| | 🟢 `get_safety_settings` / `get_audit_log` | Limits in force / log of every mutating call |
| Margins | 🟢 `get_order_margins` | Margin per order (SPAN, exposure, premium) + charges |
| | 🟢 `get_basket_margins` | Combined margin with spread/hedge benefit |
| | 🟢 `get_order_charges` | Brokerage, STT, exchange, SEBI, stamp duty, GST |
| GTT | 🟢 `get_gtts` / `get_gtt` | GTT triggers |
| | 🔴 `place_gtt` / `modify_gtt` / `delete_gtt` | Single trigger or two-leg OCO (target + stop-loss) |
| Alerts | 🟢 `get_alerts` / `get_alert` / `get_alert_history` | Price alerts |
| | 🔴 `create_alert` / `modify_alert` / `delete_alerts` | Simple alerts, or ATO alerts that place an order |
| Market | 🟢 `search_instruments` / `get_instrument` | Stocks, futures, options by symbol/name/expiry/strike |
| | 🟢 `get_quote` / `get_ohlc` / `get_ltp` | Quotes (up to 500; LTP up to 1000) |
| | 🟢 `get_historical_data` | OHLCV(+OI) candles; long ranges auto-split |
| | 🟢 `get_option_chain` | Strikes around ATM with CE/PE LTP, OI, volume, bid/ask |
| | 🟢 `market_status` | Session hours and open/closed state (IST) |
| Live | 🟢 `subscribe_ticks` / `unsubscribe_ticks` | Stream up to 3000 instruments over WebSocket |
| | 🟢 `get_live_ticks` / `ticker_status` | Latest tick per instrument (LTP, OHLC, OI, depth) |
| | 🟢 `get_order_updates` | Real-time order status updates from the stream |
| Mutual funds | 🟢 `get_mf_holdings` / `get_mf_orders` / `get_mf_order` / `get_mf_sips` | MF portfolio, orders and SIPs |
| | 🟢 `search_mf_instruments` | Fund master: NAV, plan, scheme type, minimums |

### Plan limits

- **Free (Personal) plan:** everything except market data works — account, portfolio,
  orders, GTT, alerts, mutual funds, margin and charge calculators.
- **Paid (Connect) plan** adds market data: `get_quote`, `get_ohlc`, `get_ltp`,
  `get_historical_data`, `get_option_chain` and the live ticker. On the free plan these
  return a clear "needs a paid plan" error.
- **Placing orders** also needs a **static IP** whitelisted on your Kite Connect app
  (SEBI rules for API orders).
- Kite Connect no longer offers mutual fund order or SIP placement, so MF is read-only.

## Safety

Order, GTT, position and ATO-alert tools move real money. By default:

1. They **validate** the order: the instrument exists, quantity is a multiple of the lot
   size, and the price fields match the order type. There are **no amount, quantity or
   daily-count limits**.
2. They return a **preview** (summary, estimated value, margin and charges) with a
   `confirm_token`. **Nothing is sent to Kite** until `confirm_action` is called with that
   token, within 2 minutes. Claude is instructed to only confirm after you approve.
3. Every executed, failed or dry-run action is appended to
   `~/.config/kite-mcp/audit.jsonl`, and every order is tagged `kitemcp`.

Configure in `config.toml` (see [config.example.toml](config.example.toml)):

| `[safety]` key | Default | Effect |
|---|---|---|
| `read_only` | `false` | Don't register any tool that changes anything |
| `require_confirmation` | `true` | Preview + `confirm_action` two-step flow |
| `dry_run` | `false` | Validate and log, never send to Kite |
| `allowed_exchanges` | `[]` (all) | Optional, e.g. `["NSE", "BSE"]` |
| `allowed_products` | `[]` (all) | Optional, e.g. `["CNC"]` for delivery only |
| `blocked_symbols` | `[]` (none) | Optional tradingsymbols that can never be traded |
| `order_tag` | `"kitemcp"` | Tag added to every order |


## Setup

1. Create a Kite Connect app at <https://developers.kite.trade> and set its
   **Redirect URL** to `http://127.0.0.1:5005/callback`.
2. Put your credentials in `~/.config/kite-mcp/config.toml`:

   ```bash
   mkdir -p ~/.config/kite-mcp
   cp config.example.toml ~/.config/kite-mcp/config.toml
   chmod 600 ~/.config/kite-mcp/config.toml
   ```

   Or set `KITE_API_KEY` / `KITE_API_SECRET` in the environment.
3. Build: `cargo build --release`
4. Register it:

   - **Claude Desktop (chat)** — `~/Library/Application Support/Claude/claude_desktop_config.json`:

     ```json
     {
       "mcpServers": {
         "kite": { "command": "/Users/<you>/Desktop/kite-mcp/target/release/kite-mcp" }
       }
     }
     ```

   - **Claude Code** — `claude mcp add kite -- "$PWD/target/release/kite-mcp"`

   Restart Claude afterwards.
5. Ask Claude to "log in to Kite", open the link and sign in. Or log in from a terminal:
   `./target/release/kite-mcp login`. The session is saved to
   `~/.config/kite-mcp/session.json` (mode 600) and reused until it expires (~06:00 IST).

## Configuration

| Key / env var | Default |
|---|---|
| `api_key` / `KITE_API_KEY` | — |
| `api_secret` / `KITE_API_SECRET` | — |
| `callback_port` / `KITE_CALLBACK_PORT` | `5005` |
| `ticker_url` | `wss://ws.kite.trade` |
| `[safety]` | see [Safety](#safety) |
| config dir / `KITE_MCP_HOME` | `~/.config/kite-mcp` |
| log level / `KITE_MCP_LOG` | `info` (logs go to stderr) |

Files in the config dir: `config.toml`, `session.json`, `instruments.csv` (daily instrument
cache, refreshed after 08:30 IST) and `audit.jsonl`.

## Development

```bash
cargo test
cargo fmt && cargo clippy --all-targets -- -D warnings
```

Tests run the real binary over stdio against a mocked Kite API (`wiremock`) and a local
WebSocket server; no account needed.
