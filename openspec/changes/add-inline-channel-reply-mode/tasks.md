## 1. Configuration

- [x] 1.1 Add a Discord normal-channel reply mode enum with values `thread` and `inline`, defaulting to `thread`.
- [x] 1.2 Parse and validate the new config field in TOML without changing existing deployments.
- [x] 1.3 Add Helm value rendering for the new Discord reply mode.
- [x] 1.4 Update config reference and Discord/Helm documentation.

## 2. Discord Routing

- [x] 2.1 Update normal-channel Discord message routing to select thread creation or inline channel routing based on the configured mode.
- [x] 2.2 Preserve existing routing for Discord threads and DMs regardless of the new mode.
- [x] 2.3 Use a deterministic inline session key that does not collide with thread-created session keys.
- [x] 2.4 Keep existing allowlist, mention, trusted bot, and multibot gating behavior unchanged.

## 3. Inline Reply Delivery

- [x] 3.1 Extend the router message context with an optional initial reply target for adapter-level replies.
- [x] 3.2 Ensure inline normal-channel replies reference the triggering Discord message without requiring the agent to emit `[[reply_to:...]]`.
- [x] 3.3 Verify streaming and non-streaming response paths both preserve the first-message reply reference or use a safe fallback.
- [x] 3.4 Ensure overflow chunks after the first reply continue as normal channel messages.

## 4. Tests

- [x] 4.1 Add config unit tests for default `thread`, explicit `inline`, and invalid values.
- [x] 4.2 Add Discord routing tests for default thread creation, explicit thread mode, inline mode, existing thread, and DM behavior.
- [x] 4.3 Add tests that inline mode sets the reply target to the triggering message.
- [x] 4.4 Add session key tests to ensure inline channel sessions and thread sessions do not collide.

## 5. Verification

- [x] 5.1 Run `cargo fmt`.
- [x] 5.2 Run targeted Rust tests for config, Discord routing, and adapter reply behavior.
- [x] 5.3 Run `cargo check`.
- [x] 5.4 Run Helm template checks for default values and inline mode values.
