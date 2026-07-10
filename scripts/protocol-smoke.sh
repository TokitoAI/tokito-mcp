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
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

health="$(curl --fail --silent --show-error --max-time 10 "$BASE_URL/v1/health")"
[[ "$health" == "ok" ]] || {
    echo "unexpected health response: $health" >&2
    exit 1
}

curl --fail --silent --show-error --max-time 15 \
    --dump-header "$TMP/headers" \
    --output "$TMP/initialize" \
    --request POST "$ENDPOINT" \
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

curl --fail --silent --show-error --max-time 15 \
    --output /dev/null \
    --request POST "$ENDPOINT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
    --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'

curl --fail --silent --show-error --max-time 15 \
    --output "$TMP/tools" \
    --request POST "$ENDPOINT" \
    --header "content-type: application/json" \
    --header "accept: application/json, text/event-stream" \
    --header "mcp-session-id: $session_id" \
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
required = {"search_symbols", "get_symbol", "list_libraries", "find_compatible", "part_offer_query"}
if not required <= names:
    raise SystemExit(f"tools/list missing: {sorted(required - names)}")
print(f"tools/list ok: {len(names)} tools")
PY
