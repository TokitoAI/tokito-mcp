#!/usr/bin/env bash
# Live smoke test against a running tokito-mcp-server.
#
# Defaults to http://127.0.0.1:8090 and the 22,756-symbol artifact built from
# CERN's kicad-symbols. Override with $TOKITO_MCP_URL.
#
# Usage:
#   ./scripts/smoke.sh                       # exits non-zero on any failure
#   TOKITO_MCP_URL=http://x:8090 ./scripts/smoke.sh

set -uo pipefail

URL="${TOKITO_MCP_URL:-http://127.0.0.1:8090}"
PASS=0
FAIL=0
FAILED_NAMES=()

# colours (skip if not a tty)
if [[ -t 1 ]]; then
    G=$'\033[32m'; R=$'\033[31m'; D=$'\033[2m'; B=$'\033[1m'; X=$'\033[0m'
else
    G=""; R=""; D=""; B=""; X=""
fi

check() {
    # check <name> <cmd...> — cmd must return 0 on pass
    local name="$1"; shift
    if "$@" > /tmp/smoke_out 2>&1; then
        printf "  ${G}✓${X} %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  ${R}✗${X} %s\n" "$name"
        sed 's/^/      /' /tmp/smoke_out | head -8
        FAIL=$((FAIL + 1))
        FAILED_NAMES+=("$name")
    fi
}

jq_eq() {
    # jq_eq <jq-expr> <expected>
    local actual; actual=$(jq -r "$1" /tmp/smoke_resp)
    [[ "$actual" == "$2" ]] || { echo "jq[$1] = $actual (expected $2)"; return 1; }
}

jq_ge() {
    local actual; actual=$(jq -r "$1" /tmp/smoke_resp)
    [[ "$actual" -ge "$2" ]] || { echo "jq[$1] = $actual (expected >= $2)"; return 1; }
}

get_json() {
    # get_json <path>  → saves status to STATUS, body to /tmp/smoke_resp
    STATUS=$(curl -s -o /tmp/smoke_resp -w "%{http_code}" "$URL$1")
}

post_mcp() {
    # post_mcp <body-json> [session-id]
    local body="$1"
    local sid_hdr=()
    [[ -n "${2:-}" ]] && sid_hdr=(-H "mcp-session-id: $2")
    STATUS=$(curl -s -o /tmp/smoke_resp -w "%{http_code}" \
        -D /tmp/smoke_hdr \
        -X POST "$URL/mcp" \
        -H "content-type: application/json" \
        -H "accept: application/json, text/event-stream" \
        "${sid_hdr[@]}" \
        -d "$body")
}

mcp_data() {
    # Pull the first `data: { ... }` line from /tmp/smoke_resp into /tmp/smoke_data
    grep '^data: {' /tmp/smoke_resp | head -1 | sed 's/^data: //' > /tmp/smoke_data
    [[ -s /tmp/smoke_data ]] || { echo "no SSE data in response"; return 1; }
    cp /tmp/smoke_data /tmp/smoke_resp
}

# Export helpers so `bash -c` subshells inherit them.
export URL
export -f jq_eq jq_ge get_json post_mcp mcp_data

echo "${B}tokito-mcp smoke test${X} ${D}→ ${URL}${X}"
echo ""

# -------------------------------------------------------------- 1. REST face
echo "${B}REST${X}"

check "GET /v1/health returns 200 ok" bash -c '
    get_json /v1/health
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    [[ "$(cat /tmp/smoke_resp)" == "ok" ]] || { echo "body: $(cat /tmp/smoke_resp)"; exit 1; }
'

check "GET /v1/manifest reports symbol_count >= 1000" bash -c '
    get_json /v1/manifest
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_ge ".symbol_count" 1000
    jq_ge ".lib_count" 50
    jq_eq ".schema_version" "1"
'

check "GET /v1/libraries returns >= 100 libs" bash -c '
    get_json /v1/libraries
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_ge "length" 100
'

check "GET /v1/search?q=opamp returns ranked results" bash -c '
    get_json "/v1/search?q=opamp&limit=5"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".query" "opamp"
    jq_ge ".total" 3
'

check "GET /v1/search?q=&limit=5 returns 400 with typed error" bash -c '
    get_json "/v1/search?q=&limit=5"
    [[ "$STATUS" == "400" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".error.code" "bad_request"
'

check "GET /v1/symbols/Device/R returns root with 2 pins" bash -c '
    get_json "/v1/symbols/Device/R"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".name" "R"
    jq_eq ".parent" "null"
    jq_eq ".body.pins | length" "2"
'

check "GET /v1/symbols/MCU_Microchip_ATmega/ATmega328P-A returns extends-resolved body" bash -c '
    get_json "/v1/symbols/MCU_Microchip_ATmega/ATmega328P-A"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".name" "ATmega328P-A"
    jq_eq ".parent[1]" "ATmega48PV-10A"
    jq_ge ".body.pins | length" "30"
'

check "GET /v1/symbols/Bogus/Nonexistent returns 404 not_found" bash -c '
    get_json "/v1/symbols/Bogus/Nonexistent"
    [[ "$STATUS" == "404" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".error.code" "not_found"
'

check "GET /v1/libraries/Device/symbols paginates" bash -c '
    get_json "/v1/libraries/Device/symbols?limit=5&offset=0"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".lib" "Device"
    jq_eq ".limit" "5"
    jq_ge ".total" 100
'

check "GET /v1/compatible (no filters) returns 400" bash -c '
    get_json "/v1/compatible"
    [[ "$STATUS" == "400" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".error.code" "bad_request"
'

check "GET /v1/compatible?pins=32&fp_pattern=TQFP returns 32-pin TQFP parts" bash -c '
    get_json "/v1/compatible?pins=32&fp_pattern=TQFP&limit=10"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_ge ".total" 5
'

check "GET /v1/compatible?pins=32&fp_pattern=TQFP&query=AVR narrows further" bash -c '
    get_json "/v1/compatible?pins=32&fp_pattern=TQFP&query=AVR&limit=10"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_ge ".total" 1
'

check "GET /v1/part-offer-query returns procurement hint" bash -c '
    get_json "/v1/part-offer-query?symbol_id=Device:R&value=330&package=R_0603&market=IN"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    jq_eq ".symbol_id" "Device:R"
    jq_eq ".procurement_query" "330 resistor, 0603 package"
    jq -e ".distributor_domains | index(\"digikey.in\")" /tmp/smoke_resp >/dev/null
'

# -------------------------------------------------------------- 2. MCP face
echo ""
echo "${B}MCP${X}"

INIT_BODY='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoke","version":"0.1"}}}'

check "MCP initialize returns serverInfo + session id" bash -c '
    post_mcp '\'"$INIT_BODY"\''
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    jq_eq ".result.serverInfo.name" "tokito-mcp"
    SID=$(grep -i "^mcp-session-id:" /tmp/smoke_hdr | awk "{print \$2}" | tr -d "\r\n")
    [[ -n "$SID" ]] || { echo "no session id"; exit 1; }
    echo "$SID" > /tmp/smoke_sid
'

# Use the session from the previous step for subsequent calls.
SID=$(cat /tmp/smoke_sid 2>/dev/null || echo "")
[[ -z "$SID" ]] && { echo "${R}aborting: no MCP session${X}"; exit 1; }

curl -s -o /dev/null -X POST "$URL/mcp" \
    -H "content-type: application/json" \
    -H "accept: application/json, text/event-stream" \
    -H "mcp-session-id: $SID" \
    -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'

check "MCP tools/list returns 7 tools" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    jq_eq ".result.tools | length" "7"
    jq -e ".result.tools | map(.name) | contains([\"search_symbols\",\"get_symbol\",\"list_libraries\",\"find_compatible\",\"part_offer_query\",\"resolve_by_mpn\",\"get_symbol_provenance\"])" /tmp/smoke_resp >/dev/null
'

check "MCP tools/call search_symbols returns opamp hits" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"search_symbols\",\"arguments\":{\"query\":\"opamp\",\"limit\":3}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    INNER=$(jq -r ".result.content[0].text" /tmp/smoke_resp)
    echo "$INNER" | jq -e ".total >= 3" >/dev/null
'

check "MCP tools/call get_symbol Device/R returns body" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"get_symbol\",\"arguments\":{\"lib\":\"Device\",\"name\":\"R\"}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    INNER=$(jq -r ".result.content[0].text" /tmp/smoke_resp)
    [[ "$(echo "$INNER" | jq -r ".name")" == "R" ]]
    [[ "$(echo "$INNER" | jq -r ".body.pins | length")" == "2" ]]
'

check "MCP tools/call get_symbol on extending child resolves parent body" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"get_symbol\",\"arguments\":{\"lib\":\"MCU_Microchip_ATmega\",\"name\":\"ATmega328P-A\"}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    INNER=$(jq -r ".result.content[0].text" /tmp/smoke_resp)
    [[ "$(echo "$INNER" | jq -r ".parent[1]")" == "ATmega48PV-10A" ]]
    PINS=$(echo "$INNER" | jq -r ".body.pins | length")
    [[ "$PINS" -ge 30 ]]
'

check "MCP tools/call list_libraries returns >= 100" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"list_libraries\",\"arguments\":{}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    INNER=$(jq -r ".result.content[0].text" /tmp/smoke_resp)
    [[ "$(echo "$INNER" | jq -r "length")" -ge 100 ]]
'

check "MCP tools/call find_compatible (32-pin TQFP AVR) returns hits" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"find_compatible\",\"arguments\":{\"pins\":32,\"fp_pattern\":\"TQFP\",\"query\":\"AVR\",\"limit\":10}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    INNER=$(jq -r ".result.content[0].text" /tmp/smoke_resp)
    [[ "$(echo "$INNER" | jq -r ".total")" -ge 1 ]]
'

check "MCP tools/call find_compatible without filters returns error" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"find_compatible\",\"arguments\":{}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    jq -e ".error" /tmp/smoke_resp >/dev/null
'

check "MCP tools/call part_offer_query returns procurement hint" bash -c '
    SID=$(cat /tmp/smoke_sid)
    post_mcp "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"part_offer_query\",\"arguments\":{\"symbol_id\":\"Device:R\",\"value\":\"330\",\"package\":\"R_0603\",\"market\":\"IN\"}}}" "$SID"
    [[ "$STATUS" == "200" ]] || { echo "status $STATUS"; exit 1; }
    mcp_data
    INNER=$(jq -r ".result.content[0].text" /tmp/smoke_resp)
    [[ "$(echo "$INNER" | jq -r ".procurement_query")" == "330 resistor, 0603 package" ]]
    echo "$INNER" | jq -e ".distributor_domains | index(\"digikey.in\")" >/dev/null
'

# -------------------------------------------------------------- summary
echo ""
TOTAL=$((PASS + FAIL))
if [[ $FAIL -eq 0 ]]; then
    echo "${G}${B}all ${TOTAL} checks passed${X}"
    exit 0
else
    echo "${R}${B}${FAIL} of ${TOTAL} checks FAILED:${X}"
    for n in "${FAILED_NAMES[@]}"; do echo "    ${R}- $n${X}"; done
    exit 1
fi
