## Context

OpenAB currently acts as a chat-to-ACP broker: Discord, Slack, and gateway adapters receive messages, normalize sender context, and forward prompts to an external ACP agent over stdio or WebSocket. The agent subprocess is intentionally launched with a cleared environment, so platform bot tokens are not available to the agent.

This is safe, but it prevents agents running through `codex-acp` from reading prior chat history when the current prompt refers to earlier messages. Codex supports MCP servers through `~/.codex/config.toml`, including Streamable HTTP servers, server instructions, bearer-token authentication, and tool allow lists. In the target deployment, OpenAB core and `codex-acp` run in separate pods, so a local stdio MCP server is not sufficient.

## Goals / Non-Goals

**Goals:**
- Provide an opt-in OpenAB-hosted Streamable HTTP MCP endpoint reachable from a separate agent pod.
- Allow operators to configure the MCP HTTP route path, including ingress-friendly app prefixes such as `/${APP_NAME}/`.
- Expose a scoped `read_current_thread` tool for bounded Discord and Slack thread/DM history retrieval.
- Preserve OpenAB's token isolation model: agents receive only an MCP bearer token, not Discord or Slack bot credentials.
- Enforce OpenAB channel/user deployment boundaries and thread/DM conversation scope.
- Provide Codex configuration and skill guidance so Codex can use the tool when prompts depend on prior chat context.

**Non-Goals:**
- Do not let agents browse arbitrary Discord guild or Slack workspace history.
- Do not expose platform raw API responses as the MCP contract.
- Do not add write/send-message tools in this change.
- Do not require `codex-acp` changes or custom ACP reverse request support.
- Do not enable the MCP server by default.

## Decisions

1. **Expose Streamable HTTP MCP from the OpenAB core pod.**

   Codex and OpenAB may run in different pods, so a stdio MCP server inside the OpenAB process would not be reachable by Codex. A Streamable HTTP MCP endpoint can be exposed through a cluster-local Kubernetes Service and configured in Codex with `[mcp_servers.openab_context]`.

   Alternative considered: implement ACP `client/*` reverse requests through `codex-acp`. This would require changes in both OpenAB and the third-party ACP adapter, and Codex would still need a tool surface that maps model tool calls to ACP reverse requests.

2. **Use bearer-token authentication at the MCP endpoint.**

   A shared Kubernetes Secret can be injected into the OpenAB core pod and agent pod. The token gates access to the context endpoint without exposing Discord or Slack bot tokens to Codex.

   Alternative considered: rely only on cluster network isolation. That is insufficient because any compromised pod in the namespace could call the endpoint.

3. **Scope reads to thread/DM context by default.**

   The tool accepts platform routing fields from OpenAB's existing `<sender_context>` (`channel`, `channel_id`, `thread_id`, `message_id`). The server validates the target against configured allowed channels and reads the current thread/DM by default. Discord normal channel history remains rejected unless `allow_discord_normal_channels` is explicitly enabled, and then only for allowed channels.

   Alternative considered: accept arbitrary channel IDs. That would turn the MCP server into a broad workspace crawler and conflict with OpenAB's explicit mention/thread conversation model.

4. **Return normalized message records.**

   Tool results use a stable OpenAB schema: message id, author id/name when available, timestamp, text, and attachment metadata. This avoids leaking platform-specific fields and keeps context compact.

5. **Document Codex usage with config plus skill instructions.**

   Codex supports MCP configuration in `~/.codex/config.toml` and skills that can be implicitly invoked based on descriptions. The implementation should document both: MCP config exposes the tool, and a small skill tells Codex when prior chat context should be fetched.

## Risks / Trade-offs

- **Risk: context endpoint becomes a data-exfiltration surface** → Mitigate with disabled-by-default config, bearer token auth, cluster-local service guidance, allowed-channel validation, thread/DM-only scope, and hard limits.
- **Risk: prompts or malicious messages induce unnecessary history reads** → Mitigate with Codex skill guidance that requests small limits and current thread only; enforce maximum limits server-side.
- **Risk: Slack and Discord history APIs differ** → Mitigate by normalizing output and implementing platform-specific fetch helpers behind one MCP tool contract.
- **Risk: MCP protocol implementation grows scope** → Mitigate with a minimal MCP surface: initialize, tools/list, tools/call, and the single initial tool.
- **Risk: token rotation interrupts agent access** → Mitigate by documenting rollout restart requirements and returning clear unauthorized errors.

## Migration Plan

1. Add config fields with defaults that keep the server disabled.
2. Add the HTTP MCP server and history helpers behind the config gate.
3. Add Helm values/templates for an optional cluster-local Service and shared token secret wiring.
4. Document Codex MCP config and recommended skill.
5. Roll out by enabling the server in OpenAB core, injecting the same token into Codex, and adding Codex MCP configuration.
6. Roll back by disabling the config flag or removing the cluster-local Service; existing chat/ACP behavior remains unchanged.

## Open Questions

- Should the first implementation include Slack history, or land Discord first with Slack as a follow-up task?
- Should MCP requests be session-bound with short-lived tokens in a later hardening phase?
- Should future read tools include targeted message lookup (`read_message`) after `read_current_thread` stabilizes?
