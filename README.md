# tokito-mcp

**A KiCad symbol catalog as an MCP server — and a REST API on the same body.**

`tokito-mcp` packs CERN's [`kicad-symbols`](https://gitlab.com/kicad/libraries/kicad-symbols) library into a single SQLite artifact and serves it over two faces:

- **MCP** (`POST /mcp`) — streamable HTTP JSON-RPC for LLM agents.
- **REST** (`GET /v1/*`) — the same queries for humans and non-MCP clients.

The production MCP endpoint is **`https://mcp.tokito.dev/mcp`**. Tokito
Desktop uses this MCP face only: it does not call the REST routes or read a
local copy of `symbols.sqlite`. The REST face remains available for operations,
diagnostics, and independent integrations.

Both faces hit the same in-process store: ~22.7k symbols across 220+ libraries, with extends-chains resolved (a child symbol's body comes back fully merged with its parent) and FTS5-ranked search.

## Workspace

```
crates/
  symbols/   # shared lib: SQLite schema, FTS5 search, extends resolver, postcard body decode
  server/    # axum: REST routes + rmcp MCP service over /mcp
  pack/      # CLI: walk kicad-symbols → emit symbols.sqlite + manifest.json + build.log
```

The server always opens catalogs read-only. Release images contain the
immutable official catalog; production may additionally mount the ingestion
service's generated catalog with `TOKITO_MCP_GENERATED_DB`. MCP never accepts
writes.

## Quick start

### Docker

```bash
docker pull ghcr.io/tokitoai/tokito-mcp:v0.1.10
docker run -p 8090:8090 ghcr.io/tokitoai/tokito-mcp:v0.1.10
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

`scripts/smoke.sh` exercises every REST endpoint and every MCP tool against the running server. Override its base URL with `TOKITO_MCP_URL=http://host:port`. `scripts/protocol-smoke.sh` performs the smaller credential-free deploy check against an exact MCP endpoint.

## MCP face

Production endpoint: `https://mcp.tokito.dev/mcp` (streamable HTTP JSON-RPC,
`mcp-session-id` header). A local server exposes the same route at
`http://127.0.0.1:8090/mcp`.

Seven tools:

| Tool | Purpose |
|------|---------|
| `search_symbols` | FTS5 ranked search across symbol name, description, keywords (`{query, limit}`) |
| `get_symbol` | Fetch a symbol by `{lib, name}` with its parent's body merged in |
| `list_libraries` | Enumerate the ~220 libraries in the artifact |
| `find_compatible` | Pin-count and footprint-pattern filter (`{pins, fp_pattern, query?, limit?}`) |
| `part_offer_query` | Build a distributor-search procurement hint for a catalog symbol (`{symbol_id?, lib?, name?, value?, package?, market?}`); does not return live pricing. See [`docs/BOM_OFFERS.md`](docs/BOM_OFFERS.md). |
| `resolve_by_mpn` | Resolve an exact manufacturer + MPN + package identity to its currently published generated symbol. |
| `get_symbol_provenance` | Fetch the DS-ViRe evidence and pipeline provenance for a generated symbol or exact revision. |

Example client config (Claude Desktop or any MCP client supporting streamable HTTP):

```json
{
  "mcpServers": {
    "tokito": {
      "url": "https://mcp.tokito.dev/mcp"
    }
  }
}
```

### Session lifetime

Sessions are ephemeral. A disconnected or abandoned MCP session expires after
60 seconds. If a request using an old `mcp-session-id` is rejected, or the
connection/server restarts, the client must perform a fresh `initialize`,
store the newly returned session ID, send `notifications/initialized`, and
retry only work that is safe to repeat. Session IDs are not durable across
deployments.

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
| `GET /v1/part-offer-query?symbol_id=&value=&package=&market=` | Distributor-search procurement hint for a catalog symbol; use `lib=&name=` instead of `symbol_id=` if preferred. See [`docs/BOM_OFFERS.md`](docs/BOM_OFFERS.md). |
| `GET /v1/generated/resolve?manufacturer=&mpn=&package=` | Exact generated-symbol identity resolve. |
| `GET /v1/generated/provenance?lib=&name=` | Provenance for a published generated symbol. |

Errors are typed: `{"error": {"code": "bad_request" | "not_found" | ..., "message": "..."}}`.

## Configuration

`tokito-mcp-server`:

| Flag | Env | Default | Purpose |
|------|-----|---------|---------|
| `--db` | `TOKITO_MCP_DB` | _(required)_ | Path to `symbols.sqlite` |
| `--generated-db` | `TOKITO_MCP_GENERATED_DB` | _(none)_ | Optional live generated-symbol SQLite catalog, opened read-only. Exact generated resolve, provenance, generated-library lookup, and search route here without an MCP restart. |
| `--addr` | `TOKITO_MCP_ADDR` | `127.0.0.1:8090` | Bind address |
| `--cache` | `TOKITO_MCP_CACHE` | `2048` | Per-process resolved-symbol cache capacity |
| `--allowed-hosts` | `TOKITO_MCP_ALLOWED_HOSTS` | _(loopback only)_ | Comma-separated `Host` authorities allowed on `/mcp` (DNS-rebinding guard). Public deployments set their real host(s), e.g. `mcp.tokito.dev,mcp.tokito.dev:9443`. Empty keeps the safe loopback default. |
| `--allowed-origins` | `TOKITO_MCP_ALLOWED_ORIGINS` | _(none)_ | Comma-separated browser origins for REST CORS **and** MCP `Origin` validation, e.g. `https://app.tokito.dev`. Empty disables both. |
| `--max-sessions` | `TOKITO_MCP_MAX_SESSIONS` | `256` | Max concurrent MCP sessions; `initialize` past this is rejected so a session loop can't grow the session map / task count unbounded. |

> **Behind a reverse proxy:** set `TOKITO_MCP_ALLOWED_HOSTS` to the public host so `/mcp` accepts proxied requests directly — that's the clean alternative to rewriting the upstream `Host` header at the proxy.

## Production deployment

Tagged releases publish `ghcr.io/tokitoai/tokito-mcp:vX.Y.Z`. Production
pulls an exact release tag on the VPS and exposes it through Cloudflare Tunnel;
it never deploys `latest`. The checked-in Compose manifest, required
configuration/secrets, health checks, external MCP smoke test, and rollback
procedure are in [`docs/deployment.md`](docs/deployment.md).

Tracing: `RUST_LOG=tokito_mcp_server=debug,tower_http=debug cargo run ...`.

## CI

Pull requests and pushes to `main` run (`.github/workflows/ci.yml`):

| Job | What it checks |
|-----|----------------|
| `package + advertised version` | Workspace version matches README/Docker tag via `scripts/check-version.py` |
| `cargo fmt` | Rust formatting |
| `cargo clippy` | Lint clean (`RUSTFLAGS=-D warnings`) |
| `cargo test` | Unit + integration tests (including generated-symbol REST/MCP and offline-sync tests) |
| `Docker health + MCP smoke` | Build image, wait for health, run `scripts/protocol-smoke.sh` (initialize, full tools/list, procurement call, generated-symbol sentinel calls) |

Release tags additionally publish the container image (`.github/workflows/release.yml`).

## Artifact format

`symbols.sqlite` is the immutable official catalog baked into the hosted
server image. Production fetches authenticated immutable generated revisions
from the Tokito Cloud control plane, verifies their identities and content
hashes, builds a complete SQLite serving pack, and atomically promotes it while
retaining the last-known-good pack on failure. MCP never mounts the writer
database or receives writer credentials. A direct read-only `generated.sqlite`
input remains only for local migration compatibility. Tokito Desktop consumes
catalogs through the hosted MCP and does not open either artifact directly.
Schema lives in
[`crates/symbols/src/schema.sql`](crates/symbols/src/schema.sql). Symbol bodies
(pins, graphics, fp_filters) are stored as compact
[postcard](https://crates.io/crates/postcard) blobs decoded lazily; an FTS5
virtual table backs `search_symbols`.

Alongside `symbols.sqlite`, `pack` writes:

- `manifest.json` — `{schema_version, source_commit, symbol_count, lib_count, generated_at, generator_version}`
- `build.log` — ingest errors, dangling `extends` references, top libraries by symbol count

## Performance notes

- The resolver caches fully-resolved (extends-merged) symbols in a bounded `moka` cache. Moka uses TinyLFU-style admission/eviction rather than strict LRU; the default `--cache 2048` covers the working set for typical agent workloads.
- FTS5 search is single-digit ms on the bundled artifact; `find_compatible` filters scale linearly in matches but cap at `limit`.
- The server is single-process with independent handles for the official and
  optional generated serving catalogs. Production is exposed only through the
  configured Cloudflare edge at `mcp.tokito.dev`.

## Source data

The artifact is built from CERN's [`kicad-symbols`](https://gitlab.com/kicad/libraries/kicad-symbols) repository. `pack` records the source git SHA in `meta.source_commit` and stamps it into `manifest.json` so any served artifact is fully traceable to an upstream commit.

## Attribution

Catalog packs redistribute symbol data derived from the official KiCad Symbol Libraries (CC-BY-SA-4.0, with the KiCad Libraries Exception for end-user designs). See [NOTICE.md](NOTICE.md) for full attribution and license details on the KiCad-derived content served in `symbols.sqlite`.

## License

MIT — see [LICENSE](LICENSE). The bundled symbol artifact derives from upstream KiCad libraries and inherits their license terms; see [NOTICE.md](NOTICE.md) for attribution.
