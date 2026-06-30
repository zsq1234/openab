## 1. Configuration

- [x] 1.1 Add `context_mcp` configuration with `enabled`, `bind`, `token`, `default_limit`, `max_limit`, and allowed platform controls, defaulting to disabled.
- [x] 1.2 Validate context MCP configuration at startup, including rejecting enabled mode without a non-empty token.
- [x] 1.3 Wire context MCP startup and graceful shutdown into `main.rs` without changing existing Discord/Slack/Gateway startup behavior.
- [x] 1.4 Add configurable context MCP HTTP route path, defaulting to `/mcp`.

## 2. MCP Server

- [x] 2.1 Add an HTTP MCP server module that supports authenticated `initialize`, `tools/list`, and `tools/call` JSON-RPC requests over a cluster-reachable endpoint.
- [x] 2.2 Implement bearer-token authentication and unauthorized responses for missing or invalid tokens.
- [x] 2.3 Define the `read_current_thread` tool schema and normalized response schema.
- [x] 2.4 Enforce default and maximum message limits for all context reads.

## 3. Platform History Readers

- [x] 3.1 Implement Discord thread/DM history reading for allowed OpenAB conversations, returning chronological normalized messages.
- [x] 3.2 Implement Slack thread history reading through `conversations.replies`, returning chronological normalized messages.
- [x] 3.3 Reject disallowed channels and Discord normal-channel reads by default with clear tool errors.
- [x] 3.5 Add explicit opt-in support for Discord normal channel history reads.
- [x] 3.4 Include attachment metadata without downloading attachment bodies.

## 4. Deployment And Codex Documentation

- [x] 4.1 Add Helm values/templates for optional context MCP Service exposure and shared token injection.
- [x] 4.2 Update config reference documentation for `context_mcp` fields and security guidance.
- [x] 4.3 Update Codex documentation with `[mcp_servers.openab_context]` Streamable HTTP configuration for separate pods.
- [x] 4.4 Add a recommended Codex skill snippet that tells Codex when to call `read_current_thread` and how to use `<sender_context>`.

## 5. Verification

- [x] 5.1 Add unit tests for config defaults, validation, and token authentication.
- [x] 5.2 Add MCP protocol tests for `tools/list`, valid tool calls, invalid tool calls, and limit clamping.
- [x] 5.3 Add Discord and Slack history normalization tests using fixture responses or mocked helpers.
- [x] 5.4 Run `cargo fmt`, `cargo check`, targeted tests, and Helm template checks for chart changes.
