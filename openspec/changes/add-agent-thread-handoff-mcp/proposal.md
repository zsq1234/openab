## Why

Inline normal-channel replies make a shared channel usable as a quick entry point, but complex tasks still run inside the shared channel session and can block or pollute that conversation. Agents need a controlled way to decide that a request should become its own thread-backed session while preserving the original user message as the platform thread anchor.

## What Changes

- Add an OpenAB-hosted MCP action tool that lets an agent hand off the current normal-channel message into a new thread task.
- Create the thread under the original triggering user message, reusing the same platform behavior as `normal_channel_reply_mode = "thread"`.
- Start a new ACP session for the created thread using an agent-supplied task prompt, then return immediately without waiting for that thread session to complete.
- Protect the action with a short-lived, single-use handoff token bound server-side to the current inbound message and route.
- Inject the OpenAB MCP server into ACP sessions when enabled so agents can discover and call the handoff tool.
- Keep existing behavior unchanged unless the MCP server and handoff feature are explicitly enabled.

## Capabilities

### New Capabilities

- `agent-thread-handoff`: Agent-initiated handoff from an inline normal-channel session to a new thread-backed task session anchored on the original user message.

### Modified Capabilities

None.

## Impact

- `src/context_mcp.rs`: add a mutating MCP tool, request/response schemas, token validation, and action dispatch.
- `src/adapter.rs`, `src/discord.rs`, `src/slack.rs`: reuse thread creation semantics and produce usable thread references/links for MCP responses.
- `src/dispatch.rs` and adapter message handling: carry handoff token context and enqueue background thread work with the supplied prompt.
- `src/acp/connection.rs`: pass configured OpenAB MCP server definitions in `session/new` and `session/load`.
- `src/config.rs`, docs, and Helm values: add opt-in configuration for handoff and MCP injection without changing defaults.
- Tests: cover token scoping, thread creation routing, async enqueue behavior, and disabled/default behavior.
