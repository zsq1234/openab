## ADDED Requirements

### Requirement: Context MCP server is opt-in
The system SHALL provide a configurable Streamable HTTP MCP server for chat context tools, and it SHALL be disabled by default.

#### Scenario: Disabled by default
- **WHEN** OpenAB starts without context MCP configuration
- **THEN** no context MCP listener is started

#### Scenario: Enabled with bind address
- **WHEN** context MCP is enabled with a bind address
- **THEN** OpenAB starts an HTTP MCP listener on that address

#### Scenario: Enabled with route path
- **WHEN** context MCP is enabled with a configured route path such as `/${APP_NAME}/`
- **THEN** OpenAB accepts MCP HTTP requests on that path

### Requirement: MCP endpoint requires bearer authentication
The system SHALL require a configured bearer token for all context MCP requests.

#### Scenario: Missing token rejected
- **WHEN** a request reaches the MCP endpoint without an Authorization bearer token
- **THEN** the system rejects the request with an unauthorized response

#### Scenario: Invalid token rejected
- **WHEN** a request reaches the MCP endpoint with a bearer token that does not match configuration
- **THEN** the system rejects the request with an unauthorized response

#### Scenario: Valid token accepted
- **WHEN** a request reaches the MCP endpoint with the configured bearer token
- **THEN** the system processes the MCP request

### Requirement: Tool list exposes scoped context tools
The system SHALL expose a `read_current_thread` MCP tool that describes bounded retrieval of the current Discord or Slack thread/DM history.

#### Scenario: Tools list includes read current thread
- **WHEN** an authenticated MCP client calls `tools/list`
- **THEN** the response includes `read_current_thread` with an input schema for platform, channel id, optional thread id, optional message id, and limit

### Requirement: Read current thread returns normalized messages
The system SHALL return normalized message records from `read_current_thread`, including platform, channel id, thread id, message id, author id, author name when available, timestamp, text, and attachment metadata.

#### Scenario: Discord thread history read
- **WHEN** an authenticated MCP client calls `read_current_thread` for an allowed Discord thread with a valid limit
- **THEN** the system returns up to that limit of normalized messages from the Discord thread in chronological order

#### Scenario: Slack thread history read
- **WHEN** an authenticated MCP client calls `read_current_thread` for an allowed Slack thread with a valid limit
- **THEN** the system returns up to that limit of normalized messages from the Slack thread in chronological order

#### Scenario: Empty thread result
- **WHEN** the platform API returns no messages for the scoped thread
- **THEN** the system returns an empty messages array rather than an error

### Requirement: Context reads are bounded
The system SHALL enforce a configured maximum message limit for context MCP reads.

#### Scenario: Limit above maximum is clamped
- **WHEN** an MCP client requests more messages than the configured maximum
- **THEN** the system reads no more than the configured maximum

#### Scenario: Missing limit uses default
- **WHEN** an MCP client omits the limit parameter
- **THEN** the system uses the configured default limit

### Requirement: Context reads respect OpenAB scope
The system SHALL reject context reads outside OpenAB's configured channel and conversation scope.

#### Scenario: Disallowed channel rejected
- **WHEN** an MCP client requests history for a channel not allowed by OpenAB configuration
- **THEN** the system rejects the tool call with a scoped authorization error

#### Scenario: Discord normal channel rejected by default
- **WHEN** an MCP client requests history for a Discord normal channel without a thread or DM context
- **THEN** the system rejects the tool call by default

#### Scenario: Discord normal channel accepted when explicitly enabled
- **WHEN** `allow_discord_normal_channels` is enabled and an MCP client requests history for an allowed Discord normal channel
- **THEN** the system returns bounded normalized messages from that channel

#### Scenario: Current thread accepted
- **WHEN** an MCP client requests history for a thread or DM that belongs to an allowed OpenAB conversation
- **THEN** the system allows the scoped read

### Requirement: Codex deployment is documented
The system SHALL document how to configure Codex in a separate pod to call the OpenAB context MCP endpoint.

#### Scenario: Codex config documented
- **WHEN** an operator reads the Codex/OpenAB documentation
- **THEN** the documentation includes a `[mcp_servers.openab_context]` Streamable HTTP example using a bearer token environment variable

#### Scenario: Codex skill documented
- **WHEN** an operator reads the Codex/OpenAB documentation
- **THEN** the documentation includes recommended skill guidance for fetching chat history only when prior context is needed
