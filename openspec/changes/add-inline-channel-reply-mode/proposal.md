## Why

Discord normal-channel mentions currently create or reuse a thread before the agent responds. That remains the right default for keeping long agent work out of busy channels, but some deployments want a lighter interaction mode where `@bot` replies directly in the current channel and references the triggering user message.

## What Changes

- Add an opt-in Discord reply mode for normal-channel mentions that responds in the current channel instead of creating a thread.
- Preserve the existing default behavior: normal-channel `@bot` continues to create a thread/session unless the new mode is explicitly configured.
- Ensure inline replies reference the triggering Discord message so channel readers can see which user message the bot is answering.
- Keep existing behavior for Discord threads and DMs unchanged.
- Document the new configuration and Helm value.

## Capabilities

### New Capabilities
- `discord-inline-channel-reply-mode`: Configurable Discord normal-channel mention behavior, including the existing thread-creating default and a new inline current-channel reply mode.

### Modified Capabilities

None.

## Impact

- Affected code: `src/config.rs`, `src/discord.rs`, possibly shared routing/session helpers in `src/adapter.rs` or `src/dispatch.rs`.
- Affected docs: `docs/config-reference.md`, Discord/Helm chart documentation, and chart values/templates under `charts/openab/`.
- Affected behavior: Discord normal-channel mentions only when the new mode is explicitly enabled.
- Dependencies: none expected.
