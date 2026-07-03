## Why

Agent-initiated handoff moves complex work into a thread, but the original normal-channel message currently only receives a start acknowledgement. Users watching the normal channel need a concise completion result without opening the thread, while the full discussion should remain isolated in the thread.

## What Changes

- Add an opt-in parent-channel completion reply for agent thread handoff.
- After the handoff child task's first ACP turn completes, generate a concise summary using the original normal-channel agent/session and reply to the original normal-channel message.
- Strip tool display, thinking/progress text, and other agent-internal scaffolding before sending the child result to the summarization prompt.
- Send the parent-channel completion reply only once for the initial handoff child turn; follow-up conversation inside the thread does not notify the parent channel.
- Preserve the existing asynchronous `handoff_to_thread` MCP contract: the tool still returns after thread creation and enqueue, without waiting for the child result.
- Keep the feature disabled by default and backward compatible.

## Capabilities

### New Capabilities
- `handoff-parent-summary-reply`: Controls opt-in summarized completion replies from handoff child tasks back to the original normal-channel message.

### Modified Capabilities

## Impact

- Affects `src/handoff.rs`, `src/dispatch.rs`, `src/adapter.rs`, and configuration parsing/validation.
- May require a small internal result type or hook so dispatch can observe sanitized final child content after it is sent to the child thread.
- Uses the existing ACP session machinery for the normal-channel agent to generate the summary, rather than adding a new model provider dependency.
- Requires tests for one-shot notification behavior, sanitization before summarization, disabled defaults, success and failure paths, and Discord/Slack parent reply routing.
