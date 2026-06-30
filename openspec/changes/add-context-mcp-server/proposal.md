## Why

OpenAB can already receive Discord and Slack messages and forward them to ACP agents, but agents running in a separate pod cannot safely fetch prior chat context on demand. Codex via `codex-acp` supports MCP tools, so exposing a scoped OpenAB context MCP endpoint lets the agent retrieve relevant thread history only when it needs it without leaking platform bot tokens.

## What Changes

- Add an optional OpenAB-hosted Streamable HTTP MCP server for context tools with a configurable HTTP route path.
- Provide a `read_current_thread` tool that returns bounded, structured message history for the current Discord or Slack thread/DM.
- Secure the MCP endpoint with a bearer token suitable for Kubernetes Secret injection into both the OpenAB core pod and agent pod.
- Scope reads to OpenAB-configured allowed channels and to thread/DM conversation boundaries by default.
- Document Codex configuration using `[mcp_servers.openab_context]` and a recommended skill that tells Codex when to use the tool.
- No breaking changes: the MCP server is opt-in and disabled by default.

## Capabilities

### New Capabilities
- `context-mcp-server`: OpenAB exposes scoped chat-context retrieval tools over MCP for agents running outside the OpenAB process or pod.

### Modified Capabilities
- None.

## Impact

- Affected code: configuration parsing, main process startup, new HTTP/MCP server module, Discord/Slack history read helpers, Helm values/templates, and docs for Codex/OpenAB deployment.
- Affected APIs: new optional internal MCP endpoint and MCP tool schemas.
- Affected systems: Kubernetes deployments with separate OpenAB core and Codex agent pods can connect over a cluster-local Service.
- Dependencies: may require adding a small HTTP server stack or reusing existing async HTTP dependencies; implementation should avoid exposing the endpoint publicly by default.
