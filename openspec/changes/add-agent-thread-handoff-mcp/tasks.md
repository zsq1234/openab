## 1. Configuration And Wiring

- [x] 1.1 Add disabled-by-default configuration for agent thread handoff, including enable flag, token TTL, and any MCP tool exposure controls.
- [x] 1.2 Add configuration for injecting the OpenAB MCP server into ACP sessions, including endpoint URL, bearer token source, and enabled tool list.
- [x] 1.3 Validate handoff and MCP injection configuration at startup without changing existing defaults.
- [x] 1.4 Wire shared handoff state into adapter startup and the context MCP server only when configured.

## 2. Handoff Token State

- [x] 2.1 Define a handoff token store with short-lived single-use entries bound to platform, channel route, trigger message, sender context, original prompt, and expiry.
- [x] 2.2 Mint handoff tokens only for eligible inline normal-channel messages.
- [x] 2.3 Include the handoff token in agent-visible context for eligible parent turns.
- [x] 2.4 Reject expired, unknown, reused, or ineligible tokens and remove consumed tokens atomically.

## 3. MCP Tool Contract

- [x] 3.1 Add `handoff_to_thread` to MCP `tools/list` only when agent thread handoff is enabled.
- [x] 3.2 Define the tool input schema for `handoff_token`, `title`, and `prompt`, with validation for empty or oversized values.
- [x] 3.3 Define normalized success output containing `status`, platform, channel/thread route, and best-effort link or display reference.
- [x] 3.4 Return structured MCP errors for invalid tokens, platform thread creation failures, and dispatcher enqueue failures.
- [x] 3.5 Update MCP instructions/tool description so parent agents acknowledge successful handoff instead of continuing the delegated task.

## 4. Thread Creation And Dispatch

- [x] 4.1 Reuse existing Discord thread creation behavior so handoff creates or resolves a thread under the original triggering user message.
- [x] 4.2 Reuse existing Slack thread routing behavior so handoff targets the thread rooted at the original triggering message timestamp.
- [x] 4.3 Build a synthetic child `BufferedMessage` using the agent-supplied prompt and the original sender/trigger metadata.
- [x] 4.4 Add broker-controlled handoff context to the child prompt content for auditability.
- [x] 4.5 Submit the child task through the existing dispatcher and return from the MCP call after enqueue succeeds.
- [x] 4.6 Ensure child ACP responses are routed to the child thread and are not proxied through the MCP response.

## 5. ACP MCP Injection

- [x] 5.1 Extend ACP session creation/loading to pass configured MCP server definitions instead of always sending an empty `mcpServers` list.
- [x] 5.2 Keep ACP MCP injection disabled by default and preserve current session behavior when no MCP servers are configured.
- [x] 5.3 Ensure the agent subprocess still starts with `env_clear()` and does not receive Discord, Slack, or OpenAB platform credentials through environment leakage.

## 6. Documentation And Helm

- [x] 6.1 Update `docs/config-reference.md` with agent thread handoff and MCP injection configuration.
- [x] 6.2 Update Discord and Slack docs to describe agent-initiated handoff from inline normal-channel sessions.
- [x] 6.3 Update Helm values/templates for the new configuration and shared MCP token wiring.
- [x] 6.4 Add operator guidance for prompt/tool instructions that tell agents when to call `handoff_to_thread`.

## 7. Verification

- [x] 7.1 Add config parsing and validation tests for disabled defaults and enabled handoff configuration.
- [x] 7.2 Add handoff token store tests for scope binding, expiry, and single-use consumption.
- [x] 7.3 Add MCP protocol tests for `tools/list`, successful `handoff_to_thread`, invalid tokens, and failure responses.
- [x] 7.4 Add Discord and Slack routing tests proving the original message anchors the child thread route.
- [x] 7.5 Add dispatcher tests proving the MCP call returns after enqueue and child output routes to the thread session.
- [x] 7.6 Run `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`, and Helm template checks for chart changes.
