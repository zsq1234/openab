#!/usr/bin/env bash
set -euo pipefail

MCP_URL="${MCP_URL:-http://localhost:18080/mcp}"
MCP_TOKEN="${MCP_TOKEN:-}"
MCP_PLATFORM="${MCP_PLATFORM:-discord}"
MCP_CHANNEL_ID="${MCP_CHANNEL_ID:-}"
MCP_THREAD_ID="${MCP_THREAD_ID:-}"
MCP_LIMIT="${MCP_LIMIT:-20}"

if [[ -z "$MCP_TOKEN" ]]; then
  echo "error: set MCP_TOKEN to the [context_mcp].token value" >&2
  exit 2
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required" >&2
  exit 2
fi

if command -v jq >/dev/null 2>&1; then
  pretty=(jq)
else
  pretty=(cat)
fi

rpc() {
  local payload="$1"
  curl -sS "$MCP_URL" \
    -H "Authorization: Bearer $MCP_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$payload" | "${pretty[@]}"
}

echo "== initialize =="
rpc '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-06-18",
    "capabilities": {},
    "clientInfo": {
      "name": "openab-curl-smoke",
      "version": "0.1.0"
    }
  }
}'

echo
echo "== tools/list =="
rpc '{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/list",
  "params": {}
}'

if [[ -n "$MCP_CHANNEL_ID" ]]; then
  echo
  echo "== tools/call read_current_thread =="
  if [[ -n "$MCP_THREAD_ID" ]]; then
    thread_arg=", \"thread_id\": \"$MCP_THREAD_ID\""
  else
    thread_arg=""
  fi

  rpc "{
    \"jsonrpc\": \"2.0\",
    \"id\": 3,
    \"method\": \"tools/call\",
    \"params\": {
      \"name\": \"read_current_thread\",
      \"arguments\": {
        \"platform\": \"$MCP_PLATFORM\",
        \"channel_id\": \"$MCP_CHANNEL_ID\"$thread_arg,
        \"limit\": $MCP_LIMIT
      }
    }
  }"
else
  echo
  echo "skip read_current_thread: set MCP_CHANNEL_ID to exercise history reads"
fi
