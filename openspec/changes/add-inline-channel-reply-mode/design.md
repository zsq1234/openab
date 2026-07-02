## Context

OpenAB's Discord adapter currently treats a normal-channel `@bot` mention as a trigger to create or reuse a Discord thread from the triggering message. The agent turn then runs against the thread channel, and follow-up messages can continue inside that thread. This keeps long agent exchanges out of busy channels and must remain the default.

Some operators want a lighter mode for channels where creating a thread for every mention is noisy or undesirable. In that mode, the bot should process the message but reply directly in the current channel, using Discord's message reference behavior so the reply points at the user's triggering message.

Relevant existing pieces:
- `detect_thread()` determines whether a Discord channel is a thread using `thread_metadata`, not `parent_id`.
- `get_or_create_thread()` creates a new thread for normal-channel messages.
- `ChannelRef` routes Discord sends by channel ID.
- `send_message_with_reply()` already models Discord message-reference replies and is used for `[[reply_to:<message_id>]]` output directives.
- `SenderContext` already includes `message_id`, `channel_id`, and `thread_id` metadata for the agent.

## Goals / Non-Goals

**Goals:**
- Preserve current thread-creating behavior as the default.
- Add an opt-in Discord configuration mode for normal-channel mentions that routes the agent turn to the current channel.
- Make the bot's visible response reference the triggering user message in inline mode.
- Keep thread and DM behavior unchanged.
- Keep session isolation coherent so inline normal-channel conversations do not accidentally share state with thread-created conversations unless that is an intentional mode decision.
- Document config and Helm values.

**Non-Goals:**
- Do not change Slack behavior.
- Do not remove or deprecate Discord thread creation.
- Do not add a new platform-wide reply API beyond the existing `send_message_with_reply` abstraction unless implementation proves it necessary.
- Do not change Discord thread detection rules.
- Do not let non-mentioned normal-channel messages trigger the bot.

## Decisions

1. **Use an explicit enum config instead of a boolean.**

   Add a Discord config field such as `normal_channel_reply_mode` with values:
   - `"thread"`: current behavior; create or join a thread for normal-channel mentions.
   - `"inline"`: process in the current channel and reply to the triggering message.

   This keeps defaults backward-compatible and leaves room for future modes without overloading a boolean.

   Alternative considered: `inline_channel_replies = true`. It is simpler, but less descriptive once more channel-response policies are needed.

2. **Apply the mode only when the incoming Discord message is in a normal guild channel.**

   Existing thread messages continue to route to their thread channel. DMs continue to route to the DM channel and skip thread creation. The new mode only changes the branch where `should_skip_thread_creation(in_thread=false, is_dm=false)` currently returns false.

   Alternative considered: let inline mode also affect existing thread replies. That would be surprising and would break current thread conversations.

3. **Route inline turns to the normal channel and force the first response to reference the trigger message.**

   Inline mode should build `thread_channel` as the current Discord channel. The first final/placeholder reply should use Discord message reference semantics against the triggering message ID. The implementation can reuse `send_message_with_reply()` and existing `reply_to` directive handling, either by passing an initial reply target through `AdapterRouter::handle_message` or by injecting an internal reply directive into the turn output path.

   The cleaner implementation is to extend the router context with an optional `reply_to_message_id` for adapter-level replies. This avoids relying on the model to emit `[[reply_to:...]]`.

4. **Use a distinct session key for inline channel mode.**

   Thread-created sessions are keyed by the created Discord thread channel. Inline mode has no thread channel, so use a deterministic key rooted in the normal channel, for example `discord-inline:<channel_id>` or `discord:<channel_id>:inline`. This prevents inline turns in the same channel from colliding with future thread-created sessions and makes the mode explicit in logs.

   Trade-off: all inline interactions in a channel share a session. That matches "current conversation" at the Discord channel level, but can mix independent user mentions in busy channels. A later enhancement could add per-message or per-user inline session policies if needed.

5. **Do not change `<sender_context>` schema.**

   For inline normal-channel messages, `thread_id` remains absent and `message_id` identifies the triggering message. This is already how non-thread Discord messages are represented. The new behavior is routing/reply policy, not a new context schema.

## Risks / Trade-offs

- **Risk: inline channel sessions mix unrelated requests in busy channels** → Mitigate by making inline mode opt-in and documenting that it is best for lower-volume channels or teams that want channel-level continuity.
- **Risk: Discord replies become noisy in busy channels** → Mitigate by preserving thread mode as the default.
- **Risk: streaming placeholder handling may not reference the trigger message** → Mitigate by adding tests around inline mode's first outbound Discord message and by routing the reply target through broker code rather than relying on model output.
- **Risk: multi-agent channels may produce interleaved inline replies** → Mitigate with existing mention gating and trusted-bot controls; document that thread mode remains recommended for multi-agent collaboration.
- **Risk: config naming ambiguity** → Mitigate with an enum whose default value is `"thread"` and documentation that explicitly describes each mode.

## Migration Plan

1. Add the config field with default `"thread"` so existing deployments do not change.
2. Add Helm value rendering for the new field.
3. Update Discord message routing to choose thread creation or inline channel routing based on the mode.
4. Ensure inline replies use the triggering message ID as a Discord message reference.
5. Add tests for default thread mode, inline mode, thread/DM non-regression, and config parsing.
6. Roll back by removing the config field or setting it back to `"thread"`.

## Open Questions

- Should the first inline implementation use one session per channel, or should it support an additional session policy such as per-user or per-message?
- Should inline mode disable streaming placeholders if the placeholder cannot reliably be sent as a Discord message reference?
