## ADDED Requirements

### Requirement: Handoff Tool Availability
The system SHALL expose an MCP `handoff_to_thread` tool only when agent thread handoff is explicitly enabled.

#### Scenario: Handoff disabled by default
- **WHEN** the MCP server is enabled but agent thread handoff is not enabled
- **THEN** `tools/list` SHALL NOT include `handoff_to_thread`

#### Scenario: Handoff enabled
- **WHEN** agent thread handoff is enabled for the OpenAB MCP server
- **THEN** `tools/list` SHALL include `handoff_to_thread` with an input schema requiring a handoff token, title, and prompt

### Requirement: Handoff Token Scope
The system SHALL require a short-lived single-use handoff token that is bound server-side to the current eligible inbound message.

#### Scenario: Valid token identifies original message
- **WHEN** an inline normal-channel agent calls `handoff_to_thread` with a valid token
- **THEN** the system SHALL resolve the platform, parent channel, trigger message, sender context, and original prompt from server-side token state

#### Scenario: Invalid token rejected
- **WHEN** an agent calls `handoff_to_thread` with an unknown, expired, or already-used token
- **THEN** the system SHALL reject the tool call without creating a platform thread and without enqueueing a child task

#### Scenario: Agent cannot select arbitrary route
- **WHEN** an agent supplies a valid token and any prompt/title accepted by the schema
- **THEN** the system SHALL use only the token-bound platform route and original trigger message as the handoff target

### Requirement: Original Message Thread Anchor
The system SHALL create the handoff conversation under the original user message that triggered the parent inline session.

#### Scenario: Discord handoff creates thread from original message
- **WHEN** a Discord inline normal-channel agent successfully calls `handoff_to_thread`
- **THEN** the system SHALL create or reuse a Discord thread from the original triggering user message

#### Scenario: Slack handoff uses original message as thread root
- **WHEN** a Slack inline normal-channel agent successfully calls `handoff_to_thread`
- **THEN** the system SHALL route the child task to the Slack thread rooted at the original triggering message timestamp

### Requirement: Agent-Supplied Child Prompt
The system SHALL use the prompt supplied to `handoff_to_thread` as the task prompt for the new thread-backed ACP session.

#### Scenario: Child session receives supplied prompt
- **WHEN** `handoff_to_thread` succeeds with prompt `P`
- **THEN** the first dispatched user prompt for the child thread session SHALL contain `P`

#### Scenario: Handoff context preserved
- **WHEN** the system dispatches the child thread prompt
- **THEN** the dispatched content SHALL include broker-controlled context identifying the original sender, original message, parent channel, and handoff origin

### Requirement: Asynchronous Child Execution
The system SHALL return from `handoff_to_thread` after the thread task has been created and enqueued, without waiting for the child ACP session to finish.

#### Scenario: Tool returns after enqueue
- **WHEN** platform thread creation succeeds and dispatcher enqueue succeeds
- **THEN** the MCP tool response SHALL indicate the handoff was started and include the child thread route

#### Scenario: Child result not proxied through MCP
- **WHEN** the child thread ACP session later streams or sends its response
- **THEN** those responses SHALL be sent in the child thread conversation and SHALL NOT be included in the original MCP tool response

### Requirement: Parent Session Acknowledgement Contract
The system SHALL provide tool descriptions or session guidance instructing parent agents to acknowledge successful handoff without continuing the delegated task in the parent inline session.

#### Scenario: Successful handoff response guidance
- **WHEN** an agent receives a successful `handoff_to_thread` tool result
- **THEN** the tool result or MCP instructions SHALL provide enough child thread information for the parent agent to produce a short acknowledgement

### Requirement: Backward-Compatible Defaults
The system SHALL preserve existing behavior unless agent thread handoff and MCP injection are explicitly configured.

#### Scenario: Existing inline mode unchanged without handoff
- **WHEN** `normal_channel_reply_mode = "inline"` is configured and agent thread handoff is disabled
- **THEN** normal-channel messages SHALL continue to use the existing inline session behavior

#### Scenario: Existing thread mode unchanged
- **WHEN** `normal_channel_reply_mode = "thread"` is configured
- **THEN** normal-channel messages SHALL continue to create or reuse a thread without requiring the MCP handoff tool

### Requirement: Failure Reporting
The system SHALL report handoff setup failures to the calling MCP client without partially starting child work.

#### Scenario: Platform thread creation fails
- **WHEN** `handoff_to_thread` cannot create or resolve the platform thread
- **THEN** the tool call SHALL return an error and SHALL NOT enqueue a child ACP prompt

#### Scenario: Dispatcher enqueue fails
- **WHEN** platform thread creation succeeds but dispatcher enqueue fails
- **THEN** the tool call SHALL return an error indicating the child task was not started
