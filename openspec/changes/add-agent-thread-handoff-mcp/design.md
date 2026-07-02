## Context

OpenAB now supports inline normal-channel replies for Discord and Slack. In inline mode, a normal-channel message routes directly to the current channel and uses a channel-scoped session key such as `discord-inline:<channel_id>` or `slack:<channel_id>`. This is convenient for quick exchanges, but a complex task can occupy the shared inline session and mix unrelated work into the same conversation state.

The existing thread mode already solves task isolation by creating a platform thread under the triggering message and routing the ACP turn to a thread-scoped session. The requested behavior is to let the inline-session agent decide when a request is complex, then ask OpenAB to create that same kind of thread-backed session under the original user message. The prompt for the new session should be supplied by the agent, so it can summarize or reframe the task before handoff.

The current `context_mcp` server is read-only. This change expands the MCP surface with a narrowly scoped action tool while preserving OpenAB's security model: platform credentials stay in OpenAB, not in the agent process.

## Goals / Non-Goals

**Goals:**
- Let an agent in an inline normal-channel session create a thread-backed task session for the current user message.
- Anchor the new platform thread on the original user message, matching `normal_channel_reply_mode = "thread"` behavior.
- Use the agent-supplied prompt as the first prompt in the new thread session.
- Return from the MCP call after thread creation and dispatch enqueue, without waiting for the thread session result.
- Prevent agents from creating arbitrary thread tasks outside the current inbound message context.
- Keep all new behavior opt-in and backward compatible.
- Support Discord and Slack semantics where practical: Discord creates an actual thread from a message; Slack uses the triggering message timestamp as the thread root.

**Non-Goals:**
- Do not add a general-purpose platform send-message or channel-management MCP API.
- Do not let agents choose arbitrary platform channel IDs or message IDs for handoff.
- Do not require OpenAB to classify task complexity itself.
- Do not change default inline, thread, DM, or existing-thread behavior.
- Do not wait for or proxy the child thread agent's final answer through the parent MCP call.

## Decisions

1. **Expose a dedicated `handoff_to_thread` MCP tool.**

   The tool accepts a short-lived handoff token, a title, and the new task prompt. OpenAB validates the token, creates or resolves the platform thread under the original message, and submits the supplied prompt to the dispatcher for that thread.

   Alternative considered: let the agent emit an output directive such as `[[handoff_thread:...]]`. MCP is more explicit, gives structured success/failure, and keeps the parent response flow simple.

2. **Use server-side handoff tokens instead of trusting route parameters from the agent.**

   Each eligible inbound normal-channel message gets a token bound to platform, parent channel, triggering message, sender context, trigger `MessageRef`, original prompt, and expiry. The MCP tool receives only this token plus title/prompt. Tokens are single-use and expire quickly.

   Alternative considered: accept `platform`, `channel_id`, and `message_id` in the tool input. That would let prompt injection attempt to target other allowed channels or stale messages and would make the tool too broad.

3. **Bind handoff eligibility to inline normal-channel turns.**

   The tool is intended for the parent inline session. OpenAB should only mint handoff tokens when the message is in a normal channel and inline reply mode is active. Existing thread sessions and DMs do not need to hand off to another thread under the same message.

   Alternative considered: allow handoff from any session. That may be useful later, but it increases ambiguity around which message should anchor the new thread.

4. **Reuse adapter thread creation semantics.**

   Discord uses the same `create_thread_from_message` path as normal thread mode, including race handling for "thread already exists" when possible. Slack returns a `ChannelRef` with `thread_id` set to the triggering message timestamp, matching the thread mode route.

   Alternative considered: duplicate platform API calls inside the MCP server. Reusing adapter abstractions keeps behavior aligned with current routing and avoids a second implementation of thread creation policy.

5. **Submit background work through the existing dispatcher.**

   Once the thread `ChannelRef` is available, OpenAB builds a synthetic `BufferedMessage` for the thread route using the agent-supplied prompt and original sender context. The MCP call returns after `Dispatcher::submit` succeeds. The actual ACP turn runs asynchronously through the existing per-thread/session machinery.

   Alternative considered: call `SessionPool` directly from MCP. That would bypass batching, reactions/status, streaming, and adapter response behavior.

6. **Make the child prompt auditable.**

   The supplied prompt is the child task prompt, but OpenAB should prepend or append a broker-controlled handoff context block containing the original sender, original message text, parent channel, and handoff metadata. This preserves traceability without requiring the parent agent to include all context correctly.

7. **Inject the OpenAB MCP server into ACP sessions when configured.**

   The agent can only call the tool if `session/new` and `session/load` include an MCP server entry for OpenAB. Configuration should expose the endpoint URL, bearer token, and enabled tools. Defaults remain empty/disabled.

## Risks / Trade-offs

- **Risk: prompt injection causes unwanted thread creation** -> Mitigate with opt-in config, tool allow-listing, short-lived single-use tokens, token scope limited to the current message, and clear agent instructions that handoff is only for complex tasks.
- **Risk: parent agent continues doing the complex work after handoff** -> Mitigate with tool description and system guidance: after successful handoff, the parent response should only acknowledge the created thread.
- **Risk: duplicate handoffs create duplicate platform threads or duplicate work** -> Mitigate with single-use tokens and idempotent handling when Discord reports an existing thread for the same trigger message.
- **Risk: child task loses context from the original message** -> Mitigate by injecting broker-controlled handoff context with the original prompt and sender metadata.
- **Risk: MCP action tool broadens the security surface** -> Mitigate by keeping the tool narrow, avoiding arbitrary channel/message parameters, preserving platform token isolation, and disabling by default.
- **Risk: Slack "thread creation" is implicit, not a separate API object** -> Mitigate by returning a Slack thread route rooted at the triggering message timestamp and posting the child session response in that thread.

## Migration Plan

1. Add disabled-by-default configuration for MCP handoff and MCP server injection.
2. Mint handoff tokens only for eligible inline normal-channel messages.
3. Add `handoff_to_thread` to `tools/list` only when handoff is enabled.
4. Implement token validation, thread creation, synthetic message construction, and dispatcher enqueue.
5. Inject OpenAB MCP server definitions into ACP `session/new` and `session/load` when configured.
6. Update config reference, Discord/Slack docs, and Helm values/templates.
7. Roll back by disabling the handoff config or removing the MCP server entry; existing message handling remains unchanged.

## Open Questions

- What should the default token TTL be: 5 minutes, 10 minutes, or tied to dispatcher idle timeout?
- Should title length use the existing thread-name shortening helper unconditionally for both platforms?
- Should the MCP response include a platform permalink in the first implementation, or only structured channel/thread IDs?
