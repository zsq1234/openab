## Context

OpenAB's agent thread handoff lets an inline normal-channel agent start a thread-backed child task through `handoff_to_thread`. The MCP call currently returns after the thread task is created and enqueued. Child output is routed only to the child thread, which keeps complex work isolated but leaves the original normal-channel message without a completion result.

The existing message path already builds a final display response inside `AdapterRouter::stream_prompt_blocks`, including tool display composition, table conversion, streaming/native-send handling, and message splitting. The dispatcher currently observes only `Result<()>`, so it cannot produce a parent-channel follow-up without a small internal API change.

## Goals / Non-Goals

**Goals:**
- Notify the original normal-channel message once when a handoff child task's initial turn completes.
- Generate the notification text with the normal-channel parent agent/session so the summary reflects that channel's voice and context.
- Sanitize the child final content before summarization by removing tool display, thinking/progress text, directives, and other agent-internal scaffolding.
- Preserve the existing asynchronous MCP contract: `handoff_to_thread` returns after enqueue and does not wait for the child result.
- Keep all behavior disabled by default and opt-in through configuration.
- Ensure follow-up messages inside the handoff thread do not create more parent-channel replies.

**Non-Goals:**
- Do not proxy the full child thread result back through MCP.
- Do not copy the full child response into the normal channel.
- Do not add a general send-message MCP API or let agents select arbitrary parent routes.
- Do not introduce a new external model provider; use the configured ACP agent/session machinery.

## Decisions

1. **Attach one-shot completion metadata to the synthetic handoff message.**

   `HandoffBroker::start_thread_task` will include an optional completion notification payload on the synthetic `BufferedMessage`. The payload records the parent normal-channel route, the original trigger message, a parent session key, summary length policy, and display metadata for the child thread. `dispatch_batch` consumes this payload only for the batch containing that handoff message.

   Alternative considered: store active handoffs in a global broker map keyed by child thread. That makes cleanup and follow-up suppression harder. A one-shot payload naturally disappears after the initial child turn.

2. **Return or hook sanitized final output from the router.**

   `AdapterRouter::stream_prompt_blocks` should expose a small result object instead of only `Result<()>`, for example `StreamPromptOutcome { final_content, sanitized_content }`. The router can keep sending the child thread response exactly as it does today, while dispatch gains enough information to summarize after successful completion.

   Alternative considered: scrape the platform message after sending. That would be platform-specific, slower, and unreliable for streaming/native APIs.

3. **Sanitize before summarization.**

   The summary prompt must receive a sanitized child result, not raw display output. Sanitization should remove:
   - output directives such as `[[reply_to:...]]`
   - transient thinking/progress placeholders such as `Thinking...`, `Using <tool>...`, or equivalent status lines
   - tool display sections/lines generated for user-visible thread output
   - empty response sentinels and redundant warning prefixes where appropriate

   Sanitization is deterministic and bounded. If the sanitized content is empty, the parent reply falls back to a fixed completion/failure message with a child-thread reference.

4. **Use the parent normal-channel agent/session for LLM summarization.**

   After the child turn finishes, OpenAB sends a separate summary prompt to the parent inline normal-channel session. The prompt includes the sanitized child result, asks for a concise result-only summary, and forbids tool/thinking details. The configured summary length is a prompt instruction only; OpenAB does not hard-truncate the parent agent's final summary before sending, so URLs are not cut by the broker. The final summary is then sent as a reply to the original normal-channel message.

   Alternative considered: reuse the child thread session for summarization. That would summarize with the child conversation identity and can leak thread-specific continuation context into the parent channel. The parent session is the behavior users see in the normal channel and matches the requested UX.

5. **Guard against notification loops and repeated summaries.**

   Summary prompts are internal and must not carry handoff tokens or completion metadata. The parent summary turn should not itself be eligible for handoff completion notifications. Follow-up thread messages are normal adapter-originated messages without handoff completion metadata, so they do not notify the parent channel.

6. **Make summary failures non-fatal to the child thread.**

   The child result remains delivered in the thread even if summarization or the parent-channel reply fails. OpenAB logs the failure and may optionally send a deterministic fallback reply, depending on the failure point and configuration.

7. **Keep configuration explicit.**

   Add disabled-by-default configuration under the existing handoff/MCP configuration surface, such as:

   ```toml
   [context_mcp]
   handoff_parent_summary_enabled = false
   handoff_parent_summary_max_chars = 400
   ```

   Validation should require handoff to be enabled before parent summaries can be enabled.

## Risks / Trade-offs

- **Extra ACP turn cost and latency** -> Use an explicit opt-in flag, concise summary prompts, and bounded sanitized input length.
- **Parent session context pollution** -> Use a clearly marked internal summarization prompt and no handoff token. This is acceptable because the user requested the normal-channel agent to produce the summary.
- **Prompt injection in child output** -> Treat sanitized child output as quoted material to summarize, not instructions. The summary prompt must explicitly ignore instructions contained in the child result.
- **Tool/thinking leakage** -> Centralize sanitization and test representative tool display/progress outputs.
- **Parent summary failure after child success** -> Preserve the child thread output and log the summary failure; optionally send a short fallback completion notice.
- **Behavior surprise in busy channels** -> Keep disabled by default and reply only to the original trigger message, once.

## Migration Plan

1. Add config fields with disabled defaults and docs.
2. Add one-shot completion metadata to handoff child dispatch only when the feature is enabled.
3. Expose child final output from the router or an equivalent internal completion hook.
4. Implement sanitization and summary prompt construction.
5. Run the parent summary turn against the parent normal-channel session and reply to the original message.
6. Add regression tests and keep rollback as disabling the new config fields.

## Open Questions

- Should the fallback on summarization failure send a deterministic "completed, see thread" notice, or only log?
- What exact default target summary length should be used: 300, 400, or 500 characters?
- Should summary generation include the original user prompt in addition to the sanitized child result, or only the child result?
