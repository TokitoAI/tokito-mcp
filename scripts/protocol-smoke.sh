#!/usr/bin/env bash
# Credential-free health + MCP handshake smoke test.

set -euo pipefail

ENDPOINT="${TOKITO_MCP_ENDPOINT:-${1:-https://mcp.tokito.dev/mcp}}"
ENDPOINT="${ENDPOINT%/}"
if [[ "$ENDPOINT" != */mcp ]]; then
    echo "expected an MCP endpoint ending in /mcp, got: $ENDPOINT" >&2
    exit 2
fi
BASE_URL="${ENDPOINT%/mcp}"
USER_AGENT="${TOKITO_MCP_USER_AGENT:-tokito/production-edge-smoke}"
PROTOCOL_VERSION="2025-03-26"
TMP="$(mktemp -d)"
cleanup() {
    if [[ -s "$TMP/session_id" ]]; then
        curl --silent --show-error --max-time 5 \
            --request DELETE "$ENDPOINT" \
            --user-agent "$USER_AGENT" \
            --header "mcp-session-id: $(cat "$TMP/session_id")" \
            --header "mcp-protocol-version: $PROTOCOL_VERSION" \
            --output /dev/null || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

health_status="$(curl --silent --show-error --max-time 10 \
    --user-agent "$USER_AGENT" \
    --dump-header "$TMP/health_headers" \
    --output "$TMP/health" \
    --write-out '%{http_code}' \
    "$BASE_URL/v1/health" || true)"
if [[ "$health_status" != "200" ]]; then
    echo "public health returned HTTP ${health_status:-transport-error}" >&2
    awk 'BEGIN {IGNORECASE=1} /^(cf-ray|cf-mitigated|server|content-type):/' \
        "$TMP/health_headers" >&2
    exit 1
fi
health="$(cat "$TMP/health")"
[[ "$health" == "ok" ]] || {
    echo "unexpected health response: $health" >&2
    exit 1
}

curl --fail --silent --show-error --max-time 15 \
    --dump-header "$TMP/headers" \
    --output "$TMP/initialize" \
    --request POST "$ENDPOINT" \
    --user-agent "$USER_AGENT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"tokito-deploy-smoke","version":"1.0"}}}'

session_id="$(
    awk 'BEGIN {IGNORECASE=1} /^mcp-session-id:/ {gsub("\r", "", $2); print $2}' "$TMP/headers"
)"
[[ -n "$session_id" ]] || {
    echo "initialize did not return mcp-session-id" >&2
    exit 1
}

python3 - "$TMP/initialize" "${TOKITO_MCP_EXPECTED_VERSION:-}" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
payload = next((line[6:] for line in lines if line.startswith("data: {")), None)
if payload is None:
    raise SystemExit("initialize response contained no JSON SSE event")
message = json.loads(payload)
info = message["result"]["serverInfo"]
if info["name"] != "tokito-mcp":
    raise SystemExit(f"unexpected server name: {info['name']}")
if sys.argv[2] and info["version"] != sys.argv[2]:
    raise SystemExit(f"server version {info['version']} != expected {sys.argv[2]}")
print(f"initialize ok: {info['name']} v{info['version']}")
PY

printf '%s\n' "$session_id" > "$TMP/session_id"

curl --fail --silent --show-error --max-time 15 \
    --output /dev/null \
    --request POST "$ENDPOINT" \
    --user-agent "$USER_AGENT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
    --header "mcp-protocol-version: $PROTOCOL_VERSION" \
    --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'

curl --fail --silent --show-error --max-time 15 \
    --output "$TMP/tools" \
    --request POST "$ENDPOINT" \
    --user-agent "$USER_AGENT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
    --header "mcp-protocol-version: $PROTOCOL_VERSION" \
    --data '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'

python3 - "$TMP/tools" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
payload = next((line[6:] for line in lines if line.startswith("data: {")), None)
if payload is None:
    raise SystemExit("tools/list response contained no JSON SSE event")
message = json.loads(payload)
names = {tool["name"] for tool in message["result"]["tools"]}
required = {
    "search_symbols", "get_symbol", "list_libraries", "find_compatible",
    "part_offer_query", "resolve_by_mpn", "get_symbol_provenance",
}
if not required <= names:
    raise SystemExit(f"tools/list missing: {sorted(required - names)}")
print(f"tools/list ok: {len(names)} tools")
PY

curl --fail --silent --show-error --max-time 15 \
    --output "$TMP/part_offer" \
    --request POST "$ENDPOINT" \
    --user-agent "$USER_AGENT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
    --header "mcp-protocol-version: $PROTOCOL_VERSION" \
    --data '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"part_offer_query","arguments":{"symbol_id":"Device:R","value":"330","package":"R_0603","market":"IN"}}}'

python3 - "$TMP/part_offer" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
payload = next((line[6:] for line in lines if line.startswith("data: {")), None)
if payload is None:
    raise SystemExit("part_offer_query response contained no JSON SSE event")
message = json.loads(payload)
if "error" in message:
    raise SystemExit(f"part_offer_query error: {message['error']}")
inner = json.loads(message["result"]["content"][0]["text"])
if inner.get("procurement_query") != "330 resistor, 0603 package":
    raise SystemExit(f"unexpected procurement_query: {inner.get('procurement_query')!r}")
domains = inner.get("distributor_domains") or []
if "digikey.in" not in domains:
    raise SystemExit(f"expected digikey.in in distributor_domains: {domains}")
print("part_offer_query ok")
PY

curl --fail --silent --show-error --max-time 15 \
    --output "$TMP/generated" \
    --request POST "$ENDPOINT" \
    --user-agent "$USER_AGENT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
    --header "mcp-protocol-version: $PROTOCOL_VERSION" \
    --data '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"resolve_by_mpn","arguments":{"manufacturer":"Tokito deploy smoke","mpn":"NOT-A-REAL-PART","package":"NONE"}}}'

python3 - "$TMP/generated" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
payload = next((line[6:] for line in lines if line.startswith("data: {")), None)
if payload is None:
    raise SystemExit("resolve_by_mpn response contained no JSON SSE event")
message = json.loads(payload)
if "error" in message:
    raise SystemExit(f"resolve_by_mpn error: {message['error']}")
inner = json.loads(message["result"]["content"][0]["text"])
if inner.get("status") != "not_found":
    raise SystemExit(f"expected generated-symbol not_found sentinel: {inner}")
print("resolve_by_mpn ok")
PY

curl --fail --silent --show-error --max-time 15 \
    --output "$TMP/provenance" \
    --request POST "$ENDPOINT" \
    --user-agent "$USER_AGENT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
    --header "mcp-protocol-version: $PROTOCOL_VERSION" \
    --data '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_symbol_provenance","arguments":{"revision_id":"gen_sha256_not_real"}}}'

python3 - "$TMP/provenance" <<'PY'
import json
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
payload = next((line[6:] for line in lines if line.startswith("data: {")), None)
if payload is None:
    raise SystemExit("get_symbol_provenance response contained no JSON SSE event")
message = json.loads(payload)
if "error" in message:
    raise SystemExit(f"get_symbol_provenance error: {message['error']}")
inner = json.loads(message["result"]["content"][0]["text"])
if inner.get("status") != "not_found":
    raise SystemExit(f"expected provenance not_found sentinel: {inner}")
print("get_symbol_provenance ok")
PY
