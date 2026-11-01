# personal-kite-mcp — Implementation Plan

A local MCP server, written in Rust, that wraps the Zerodha **Kite Connect v3** REST API (and later the WebSocket ticker).
Claude (Desktop / Code) launches it over **stdio** and calls its tools.

---

## 1. Architecture

```
Claude ──stdio(JSON-RPC)──▶ personal-kite-mcp
                              ├── mcp/        tool definitions (rmcp #[tool] macros)
                              ├── kite/       typed Kite Connect client (reqwest)
                              ├── auth/       login flow + token persistence
                              ├── safety/     order guards, limits, dry-run
                              ├── instruments/ cached instrument master + search
                              └── ticker/     (phase 5) WebSocket live ticks
```

- **Two layers**: a reusable `kite` client crate module (pure API, no MCP knowledge) and a thin `mcp` layer that maps tools → client calls. Keeps the API client testable and lets us reuse it later (CLI, bots).
- **Transport**: stdio only. Logs go to **stderr / a log file**, never stdout (stdout is the MCP channel).
- **Config**: `~/.config/personal-kite-mcp/config.toml` + env vars (`KITE_API_KEY`, `KITE_API_SECRET`). Secrets never in the repo.

## 2. Crates

| Crate | Purpose |
|---|---|
| `rmcp` 3.8 (features `server`, `transport-io`, `macros`) | Official Rust MCP SDK |
| `tokio` 1.54 (full) | Async runtime |
| `reqwest` 0.13 (`json`, `rustls`) | HTTP client |
| `serde`, `serde_json` | (De)serialization |
| `schemars` 1.2 | JSON Schema for tool inputs (required by rmcp) |
| `sha2`, `hex` | Login checksum `sha256(api_key + request_token + api_secret)` |
| `csv` 1.4 | Instruments dump parsing |
| `governor` | Per-endpoint rate limiting |
| `chrono` / `chrono-tz` | IST timestamps, token expiry (~6 AM IST daily) |
| `thiserror`, `anyhow` | Errors |
| `tracing`, `tracing-subscriber`, `tracing-appender` | Logging to stderr/file |
| `toml`, `directories` | Config + platform paths |
| `axum` (small) | Local callback server to capture `request_token` on login |
| `tokio-tungstenite` 0.30 | (Phase 5) WebSocket ticker |
| dev: `wiremock`, `insta`, `tokio-test` | Mock Kite API, snapshot tests |

## 3. Authentication flow

Kite access tokens expire daily, so login must be smooth:

1. Tool `login` → returns the Kite login URL (`https://kite.zerodha.com/connect/login?v=3&api_key=…`) and starts a one-shot local callback server on `127.0.0.1:<port>` (your Kite app's redirect URL must point there).
2. You log in in the browser → Kite redirects with `request_token` → server captures it automatically.
3. Server calls `POST /session/token` with the checksum → gets `access_token`.
4. Token saved to `~/.config/personal-kite-mcp/session.json` (file mode `0600`) with expiry; reused on restart until it expires.
5. Fallback tool `complete_login(request_token)` if you paste the token manually.
6. Any `TokenException` (403) → clear session, tell Claude to call `login` again.

## 4. Tool catalogue

Annotated with MCP hints: 🟢 read-only · 🟠 mutating · 🔴 destructive / money-moving.

### Session & account
| Tool | Endpoint | |
|---|---|---|
| `login` / `complete_login` / `logout` | `/session/token` | 🟠 |
| `session_status` | local | 🟢 |
| `get_profile` | `GET /user/profile` | 🟢 |
| `get_margins` (segment: equity/commodity/all) | `GET /user/margins[/:segment]` | 🟢 |

### Orders
| Tool | Endpoint | |
|---|---|---|
| `place_order` (variety regular/amo/co/iceberg/auction; MARKET/LIMIT/SL/SL-M; CNC/MIS/NRML/MTF; validity DAY/IOC/TTL; tag) | `POST /orders/:variety` | 🔴 |
| `modify_order` | `PUT /orders/:variety/:id` | 🔴 |
| `cancel_order` | `DELETE /orders/:variety/:id` | 🔴 |
| `cancel_all_open_orders` (optional, guarded) | loop | 🔴 |
| `get_orders` (with filters: status, symbol, tag) | `GET /orders` | 🟢 |
| `get_order_history` | `GET /orders/:id` | 🟢 |
| `get_trades` / `get_order_trades` | `GET /trades`, `/orders/:id/trades` | 🟢 |

### Portfolio
| Tool | Endpoint | |
|---|---|---|
| `get_holdings` (+ computed P&L summary) | `GET /portfolio/holdings` | 🟢 |
| `get_positions` (net / day) | `GET /portfolio/positions` | 🟢 |
| `convert_position` | `PUT /portfolio/positions` | 🔴 |
| `get_auctions` | `GET /portfolio/holdings/auctions` | 🟢 |
| `authorise_holdings` (CDSL/TPIN URL) | `POST /portfolio/holdings/authorise` | 🟠 |

### Market data
| Tool | Endpoint | |
|---|---|---|
| `search_instruments` (fuzzy by symbol/name, exchange, segment, expiry, strike, CE/PE) | cached `/instruments` CSV | 🟢 |
| `get_instrument` (token ↔ `EXCHANGE:SYMBOL`) | cache | 🟢 |
| `get_quote` (full, up to 500) | `GET /quote` | 🟢 |
| `get_ohlc` / `get_ltp` | `/quote/ohlc`, `/quote/ltp` | 🟢 |
| `get_historical_data` (interval, from/to, continuous, oi; auto-chunks long ranges) | `/instruments/historical/...` | 🟢 |
| `get_option_chain` (built from instruments + batched quotes) | composite | 🟢 |

### Margins & charges
| Tool | Endpoint | |
|---|---|---|
| `get_order_margins` | `POST /margins/orders` | 🟢 |
| `get_basket_margins` | `POST /margins/basket` | 🟢 |
| `get_order_charges` (brokerage/taxes preview) | `POST /charges/orders` | 🟢 |

### GTT (Good Till Triggered)
`place_gtt` (single / OCO two-leg) 🔴 · `modify_gtt` 🔴 · `delete_gtt` 🔴 · `get_gtts` 🟢 · `get_gtt` 🟢

### Alerts
`create_alert` 🟠 · `modify_alert` 🟠 · `delete_alerts` 🟠 · `get_alerts` 🟢 · `get_alert` 🟢 · `get_alert_history` 🟢

### Mutual funds
`place_mf_order` 🔴 · `cancel_mf_order` 🔴 · `get_mf_orders` 🟢 · `get_mf_order` 🟢 ·
`place_mf_sip` 🔴 · `modify_mf_sip` 🔴 · `cancel_mf_sip` 🔴 · `get_mf_sips` 🟢 ·
`get_mf_holdings` 🟢 · `search_mf_instruments` 🟢

### Live data (phase 5 — WebSocket)
`subscribe_ticks` (ltp/quote/full) · `unsubscribe_ticks` · `get_latest_ticks` (from in-memory cache) · `ticker_status`

### Convenience / analytics
`portfolio_summary` (holdings + positions + margins in one call) · `day_pnl` · `market_status` (NSE hours / holiday-aware)

## 5. Safety layer (important — this places real orders)

- **Read-only mode** (`read_only = true` in config) → mutating tools are not even registered.
- **Two-step orders**: `place_order` returns a **preview** (margins + charges + resolved instrument) with a `confirm_token`; a second call with that token actually places it. Configurable (`require_confirmation`).
- **Hard limits** in config: max order value, max quantity, max orders/day, allowed exchanges & products, symbol allow/deny list.
- **Dry-run mode** for testing without hitting `/orders`.
- **MCP annotations** (`destructiveHint`, `readOnlyHint`) so Claude prompts you before calling them.
- **Audit log**: every mutating call (request + response) appended to `~/.config/personal-kite-mcp/audit.jsonl`.
- Auto `tag` on every order (e.g. `kitemcp`) so they're identifiable in the order book.

## 6. Cross-cutting concerns

- **Rate limiting** (Kite limits): quotes 1 req/s, historical 3 req/s, orders 10 req/s & 200/min, others 10 req/s → `governor` limiter per bucket.
- **Errors**: map Kite `error_type` (TokenException, OrderException, MarginException, InputException, NetworkException…) to a typed enum → clear tool error messages for Claude.
- **Retries**: only for idempotent GETs on 429/5xx/NetworkException. Never auto-retry order placement.
- **Instruments cache**: download CSV once per day (all exchanges ≈ 100k rows), keep in memory + disk cache, in-memory index for search.
- **Output shaping**: return compact JSON; large lists (orders, instruments, candles) get `limit`/filter params so Claude's context isn't flooded.

## 7. Project layout

```
src/
  main.rs              # CLI args, config load, logging, start stdio server
  config.rs
  error.rs
  kite/
    mod.rs  client.rs  ratelimit.rs  models/{user,order,portfolio,market,gtt,alert,mf,margin}.rs
    api/{user,orders,portfolio,market,historical,margins,gtt,alerts,mf}.rs
  auth/{mod.rs, callback_server.rs, store.rs}
  instruments/{mod.rs, cache.rs, search.rs}
  safety/{mod.rs, limits.rs, confirm.rs, audit.rs}
  mcp/{mod.rs, server.rs, tools/{session,orders,portfolio,market,margins,gtt,alerts,mf,ticker}.rs}
  ticker/{mod.rs, parser.rs}   # phase 5
tests/                 # wiremock-based integration tests
```

## 8. Phases

| Phase | Scope | Done when |
|---|---|---|
| **1. Foundation** | deps, config, logging, error types, Kite HTTP client, rate limiter, auth flow + token store, rmcp stdio server with `login`/`session_status`/`get_profile` | Claude can log you in and read your profile |
| **2. Read-only data** | margins, holdings, positions, orders/trades listing, instruments cache + search, quotes/LTP/OHLC, historical | Claude can answer "how's my portfolio / what's RELIANCE at" |
| **3. Trading** | safety layer, place/modify/cancel order, margins & charges preview, convert position, audit log | Preview → confirm → order placed in Kite |
| **4. GTT, Alerts, MF** | all remaining endpoints | full REST coverage |
| **5. Live ticker** | WebSocket binary parser, subscription manager, tick cache tools | live LTP streaming |
| **6. Polish** | option chain, portfolio summary, README setup guide, `.mcp.json` / Claude Desktop config, CI (needs `gh auth refresh -s workflow`) | |

## 9. Testing

- Unit tests for models (deserialize real Kite JSON samples from the docs) and checksum.
- `wiremock` integration tests for every endpoint, including error envelopes.
- MCP tests: spin the server in-process and call `tools/list` / `tools/call`.
- Manual end-to-end with your account in **read-only mode first**, then a tiny LIMIT order far from market price to test place/cancel.

## 10. What you need to provide

1. A **Kite Connect app** at developers.kite.trade (paid subscription), with:
   - `api_key`, `api_secret`
   - Redirect URL set to `http://127.0.0.1:5005/callback` (port configurable)
2. Decisions:
   - Default safety: start with `require_confirmation = true` and a max order value? (suggest ₹25,000)
   - Include the WebSocket ticker (phase 5) or skip it for now?
