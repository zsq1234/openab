## ADDED Requirements

### Requirement: Discord normal-channel reply mode is configurable
The system SHALL provide a Discord configuration option that controls how normal-channel `@bot` mentions are answered, and it SHALL default to the existing thread-creating behavior.

#### Scenario: Default mode creates a thread
- **WHEN** a user mentions the Discord bot in an allowed normal guild channel and no reply mode is configured
- **THEN** the system creates or joins a Discord thread from the triggering message and processes the turn in that thread

#### Scenario: Explicit thread mode creates a thread
- **WHEN** a user mentions the Discord bot in an allowed normal guild channel and the reply mode is configured as thread mode
- **THEN** the system creates or joins a Discord thread from the triggering message and processes the turn in that thread

#### Scenario: Invalid reply mode rejected
- **WHEN** Discord configuration contains an unknown normal-channel reply mode
- **THEN** the system rejects the configuration at startup with a clear validation error

### Requirement: Inline mode replies in the current Discord channel
The system SHALL support an inline Discord normal-channel reply mode that processes `@bot` mentions in the current channel without creating a thread.

#### Scenario: Inline mode does not create thread
- **WHEN** a user mentions the Discord bot in an allowed normal guild channel and inline mode is enabled
- **THEN** the system processes the turn using the current channel as the response target
- **AND** the system does not create a Discord thread for the triggering message

#### Scenario: Inline mode references triggering message
- **WHEN** the bot sends its visible response for an inline normal-channel mention
- **THEN** the first response message references the triggering user message using Discord reply/message-reference semantics

#### Scenario: Inline mode preserves mention gating
- **WHEN** a user sends a normal-channel message that does not mention the Discord bot
- **THEN** the system does not process the message because inline mode is enabled

### Requirement: Existing Discord thread and DM behavior is unchanged
The system SHALL limit the new reply mode to normal guild channels and SHALL preserve existing routing for Discord threads and DMs.

#### Scenario: Existing thread continues in thread
- **WHEN** a user sends a message in an allowed Discord thread
- **THEN** the system processes the turn in that existing thread regardless of normal-channel reply mode

#### Scenario: DM continues in DM
- **WHEN** a user sends an allowed Discord DM to the bot
- **THEN** the system processes the turn in the DM channel regardless of normal-channel reply mode

### Requirement: Inline mode session identity is distinct
The system SHALL use a deterministic session identity for inline normal-channel conversations that does not collide with thread-created Discord sessions.

#### Scenario: Inline session key uses normal channel context
- **WHEN** inline mode processes a normal-channel mention
- **THEN** the ACP session key is derived from the Discord channel context rather than from a newly-created thread

#### Scenario: Thread sessions remain thread-scoped
- **WHEN** thread mode processes a normal-channel mention by creating or joining a thread
- **THEN** the ACP session key remains scoped to the Discord thread channel
