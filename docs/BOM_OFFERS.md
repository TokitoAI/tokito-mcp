# BOM procurement hints (`part_offer_query`)

The catalog server does **not** return live stock, pricing, or distributor SKUs. It returns **search hints** the Tokito desktop uses to drive LLM web-search offer enrichment in `bom_pricing`.

## Tool surface

| Face | Entry point |
|------|-------------|
| MCP | `tools/call` → `part_offer_query` |
| REST | `GET /v1/part-offer-query` |

### Arguments

At least one symbol identifier is required:

| Field | Required | Notes |
|-------|----------|-------|
| `symbol_id` | one of `symbol_id` or (`lib` + `name`) | Canonical `lib:name` key, e.g. `Device:R` |
| `lib` | with `name` | Library name when `symbol_id` omitted |
| `name` | with `lib` | Symbol name when `symbol_id` omitted |
| `value` | no | BOM value field (e.g. `330`, `10uF`) |
| `package` | no | Footprint/package hint (e.g. `R_0603`); falls back to resolved symbol footprint |
| `market` | no | ISO country code for distributor domain hints (`US`, `IN`, `GB`, `AU`, EU subset). Defaults to US domains. |

### Response (`PartOfferQueryResponse`)

```json
{
  "symbol_id": "Device:R",
  "value": "330",
  "package": "R_0603",
  "market": "IN",
  "procurement_query": "330 resistor, 0603 package",
  "exact_mpn": null,
  "datasheet": null,
  "description": "Resistor",
  "fp_filters": "R_*",
  "footprint": "R_0603",
  "distributor_domains": ["digikey.in", "mouser.in", "in.element14.com", "rsdelivers.com", "arrow.com"],
  "notes": [
    "The symbol catalog does not contain live stock or pricing.",
    "Use procurement_query with distributor web search, then verify electrical/package compatibility before committing a BOM offer."
  ]
}
```

`exact_mpn` is always `null` today — the catalog stores KiCad symbols, not procurement records.

## Tokito desktop flow

1. User triggers **Refresh BOM offers** in the studio.
2. `bom_pricing::refresh_design_bom_offers` resolves each BOM line's catalog symbol.
3. `tokito-catalog` calls MCP `part_offer_query` (via `CatalogGrounding::part_offer_query`).
4. The returned `procurement_query` + `distributor_domains` seed the internal LLM web-search gateway.
5. Parsed offers are upserted into the local Postgres BOM tables.

If the hosted MCP server is older and lacks the tool, the client surfaces `CatalogError::Unsupported("part_offer_query")` and BOM refresh skips that line with a warning.

## Smoke verification

**Full local smoke** (REST + all MCP tools, including `part_offer_query`):

```bash
./scripts/smoke.sh
```

**Deploy / CI protocol smoke** (credential-free handshake + tool list + `part_offer_query` call):

```bash
bash scripts/protocol-smoke.sh http://127.0.0.1:8090/mcp
```

CI runs the protocol smoke inside the Docker job (`.github/workflows/ci.yml`).

## Implementation

- Server logic: [`crates/server/src/part_offer_query.rs`](../crates/server/src/part_offer_query.rs)
- MCP handler: [`crates/server/src/mcp/server.rs`](../crates/server/src/mcp/server.rs)
- REST route: `GET /v1/part-offer-query` in [`crates/server/src/rest/search.rs`](../crates/server/src/rest/search.rs)
- Client decode/validation: [`tokito-catalog` `mcp_client.rs`](https://github.com/TokitoAI/tokito-catalog/blob/master/src/mcp_client.rs)
