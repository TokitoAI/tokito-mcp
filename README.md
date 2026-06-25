# tokito-mcp

**A KiCad symbol catalog as an MCP server — and a REST API on the same body.**

`tokito-mcp` packs CERN's [`kicad-symbols`](https://gitlab.com/kicad/libraries/kicad-symbols) library into a single SQLite artifact and serves it over two faces:

- **MCP** (`POST /mcp`) — streamable HTTP JSON-RPC for LLM agents.
- **REST** (`GET /v1/*`) — the same queries for humans and non-MCP clients.

Both faces hit the same in-process store: ~22.7k symbols across 220+ libraries, with extends-chains resolved (a child symbol's body comes back fully merged with its parent) and FTS5-ranked search.

## Workspace

```
crates/
  symbols/   # shared lib: SQLite schema, FTS5 search, extends resolver, postcard body decode
  server/    # axum: REST routes + rmcp MCP service over /mcp
  pack/      # CLI: walk kicad-symbols → emit symbols.sqlite + manifest.json + build.log
```

The server is read-only. `pack` is the only writer.

## Quick start

### Docker

```bash
docker pull ghcr.io/vtrontokito/tokito-mcp:latest
docker run -p 8090:8090 ghcr.io/vtrontokito/tokito-mcp:latest
curl http://localhost:8090/v1/health
```

The image bakes in `symbols.sqlite` — no volume mount needed.

### From source

Prereqs: Rust 1.88+. `sqlite3` is bundled via `rusqlite`'s `bundled` feature (no system dep).

```bash
# 1. Build the binaries
cargo build --release

# 2. Build the artifact from a checkout of CERN's kicad-symbols
git clone https://gitlab.com/kicad/libraries/kicad-symbols
cargo run --release -p tokito-mcp-pack -- \
    --src ./kicad-symbols \
    --out ./symbols.sqlite \
    --source-commit "$(git -C kicad-symbols rev-parse HEAD)"

# 3. Serve it
cargo run --release -p tokito-mcp-server -- --db ./symbols.sqlite

# 4. Smoke test
./scripts/smoke.sh
```

`scripts/smoke.sh` exercises every REST endpoint and every MCP tool against the running server. Override the target with `TOKITO_MCP_URL=http://host:port`.

## MCP face

Endpoint: `POST /mcp` (streamable HTTP JSON-RPC, `mcp-session-id` header).

Four tools:

| Tool | Purpose |
|------|---------|
| `search_symbols` | FTS5 ranked search across symbol name, description, keywords (`{query, limit}`) |
| `get_symbol` | Fetch a symbol by `{lib, name}` with its parent's body merged in |
| `list_libraries` | Enumerate the ~220 libraries in the artifact |
| `find_compatible` | Pin-count and footprint-pattern filter (`{pins, fp_pattern, query?, limit?}`) |

Example client config (Claude Desktop or any MCP client supporting streamable HTTP):

```json
{
  "mcpServers": {
    "tokito": {
      "url": "http://127.0.0.1:8090/mcp"
    }
  }
}
```

## REST face

All under `/v1`, all JSON:

| Endpoint | Returns |
|----------|---------|
| `GET /v1/health` | `ok` |
| `GET /v1/manifest` | `{schema_version, source_commit, symbol_count, lib_count, generated_at}` |
| `GET /v1/libraries` | `[{lib, symbol_count}, …]` |
| `GET /v1/libraries/:lib/symbols?limit&offset` | Paginated symbol list |
| `GET /v1/search?q=&limit=` | FTS5 search (`bad_request` on empty query) |
| `GET /v1/symbols/:lib/:name` | Full symbol with extends resolved (404 if missing) |
| `GET /v1/compatible?pins=&fp_pattern=&query=&limit=` | Pin+footprint filter (`bad_request` if no filter) |

Errors are typed: `{"error": {"code": "bad_request" | "not_found" | ..., "message": "..."}}`.

## Configuration

`tokito-mcp-server`:

| Flag | Env | Default | Purpose |
|------|-----|---------|---------|
| `--db` | `TOKITO_MCP_DB` | _(required)_ | Path to `symbols.sqlite` |
| `--addr` | `TOKITO_MCP_ADDR` | `127.0.0.1:8090` | Bind address |
| `--cache` | `TOKITO_MCP_CACHE` | `2048` | Per-process resolved-symbol LRU capacity |
| `--allowed-hosts` | `TOKITO_MCP_ALLOWED_HOSTS` | _(loopback only)_ | Comma-separated `Host` authorities allowed on `/mcp` (DNS-rebinding guard). Public deployments set their real host(s), e.g. `mcp.tokito.dev,mcp.tokito.dev:9443`. Empty keeps the safe loopback default. |
| `--allowed-origins` | `TOKITO_MCP_ALLOWED_ORIGINS` | _(none)_ | Comma-separated browser origins for REST CORS **and** MCP `Origin` validation, e.g. `https://app.tokito.dev`. Empty disables both. |
| `--max-sessions` | `TOKITO_MCP_MAX_SESSIONS` | `256` | Max concurrent MCP sessions; `initialize` past this is rejected so a session loop can't grow the session map / task count unbounded. |

> **Behind a reverse proxy:** set `TOKITO_MCP_ALLOWED_HOSTS` to the public host so `/mcp` accepts proxied requests directly — that's the clean alternative to rewriting the upstream `Host` header at the proxy.

Tracing: `RUST_LOG=tokito_mcp_server=debug,tower_http=debug cargo run ...`.

## Artifact format

`symbols.sqlite` is a self-contained SQLite database — the single catalog artifact, consumed by both the hosted server and the desktop app. Schema lives in [`crates/symbols/src/schema.sql`](crates/symbols/src/schema.sql). Symbol bodies (pins, graphics, fp_filters) are stored as compact [postcard](https://crates.io/crates/postcard) blobs decoded lazily; an FTS5 virtual table backs `search_symbols`.

Alongside `symbols.sqlite`, `pack` writes:

- `manifest.json` — `{schema_version, source_commit, symbol_count, lib_count, generated_at, generator_version}`
- `build.log` — ingest errors, dangling `extends` references, top libraries by symbol count

## Performance notes

- The resolver caches fully-resolved (extends-merged) symbols in a `moka` LRU. The default `--cache 2048` covers the working set for typical agent workloads.
- FTS5 search is single-digit ms on the bundled artifact; `find_compatible` filters scale linearly in matches but cap at `limit`.
- The server is single-process, single-DB, no auth — bind it to localhost or put it behind a reverse proxy if you need either.

## Source data

The artifact is built from CERN's [`kicad-symbols`](https://gitlab.com/kicad/libraries/kicad-symbols) repository. `pack` records the source git SHA in `meta.source_commit` and stamps it into `manifest.json` so any served artifact is fully traceable to an upstream commit.

## License

MIT — see [LICENSE](LICENSE). The bundled symbol artifact derives from upstream KiCad libraries and inherits their license terms.
