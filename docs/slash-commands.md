# Slash Commands

OpenAB currently exposes four Discord slash commands.

## Commands

| Command | Description | Requires active session? |
|---------|-------------|--------------------------|
| `/cancel` | Cancel the current in-flight operation | Yes |
| `/join` | Log in to Univer Workspace and join the current channel's mapped Space as an editor | No |
| `/bind` | Bind the current root channel to a Workspace Space (server owner only) | No |
| `/init` | Create and bind a same-name Workspace Space for the current root channel (server owner or channel creator) | No |

All responses are **ephemeral** — only the user who invoked the command sees the reply.
Implementations for the other historical commands remain in the codebase but
are not registered with Discord, so users do not see them in the command picker.

## Platform Support

| Platform | Supported | Notes |
|----------|-----------|-------|
| Discord | ✅ | Commands are registered globally and may take time to propagate after deployment |
| Slack | ❌ | Native Slack slash commands are not supported in threads. Send `/cancel` as a normal thread message to cancel an in-flight turn; see [slack.md](slack.md#native-slash-commands-are-not-supported-on-slack) |

## How They Work

### `/models` and `/agents`

These read `configOptions` from the ACP `initialize` / `session/new` response and present them as a Discord Select Menu.

When the user picks an option, OpenAB sends `session/set_config_option` to the ACP backend.

**Agent support varies:**

| Agent | `/models` | `/agents` |
|-------|-----------|-----------|
| openab-agent | ✅ Returns available models via `configOptions` in `session/new` response | ❌ |
| kiro-cli | ✅ Returns available models via `models` fallback | ✅ Returns modes (`kiro_default`, `kiro_planner`) via `modes` fallback |
| claude-code | ❌ No `configOptions` emitted | ❌ |
| codex | ❌ | ❌ |
| gemini | ❌ | ❌ |
| cursor-agent | ❌ (tracking: #493) | ❌ |
| copilot | ❌ (tracking: #496) | ❌ |

If the agent doesn't expose options, the user sees: `⚠️ No model options available. Start a conversation first by @mentioning the bot.`

> **Backward compatibility:** `openab-agent` returns `configOptions` in the `session/new` response (alongside `sessionId`). ACP clients that only read `sessionId` will continue to work — `configOptions` is additive. Clients that support `/models` should read `configOptions[].options` to populate the model picker. Each model option includes a `provider` field (`"anthropic"` or `"openai"`) for routing.

> **Note:** Discord Select Menus are limited to 25 items. If the agent returns more, only the first 25 are shown with a count of how many were truncated.

### `/cancel`

Sends a `session/cancel` JSON-RPC notification to the ACP backend. This aborts in-flight LLM requests and tool calls immediately — no need to wait for the current response to finish.

### `/reset`

Cancels any in-flight operation, then removes the session from the pool. The ACP process terminates once the last reference is released. The next message in the thread or DM will automatically create a fresh session.

This is equivalent to the `sessions close` + `sessions new` pattern used by [OpenClaw ACPX](https://github.com/openclaw/acpx).

**What gets cleared:**
- Conversation history
- ACP process and connection
- Suspended session state (no resume after reset)

**What is preserved:**
- Bot identity and system prompt (re-applied on next session creation)
- Config settings in `config.toml`

### `/join`

In a Discord root channel previously bound with `/bind`, `/join` logs in the
invoking Discord user through Workspace and grants that Workspace user the
`editor` role in the mapped Team Space. The response is ephemeral. If Workspace
integration is not configured or the current channel ID has no mapping, no
Workspace login or membership request is made.

### `/bind`

`/bind space_id:<id>` binds the current Discord root channel to a Workspace
Team Space. Only the Discord server owner can use it, and it is rejected in
threads and DMs. Bindings take effect immediately and persist in
`$HOME/.openab/channel_space.json` across OpenAB restarts.

### `/init`

`/init` is restricted to the Discord server owner or root channel creator and
can only be used in root channels. OpenAB verifies channel creators from
Discord's `Channel Create` audit entries, which requires the bot's **View Audit
Log** permission. Because Discord retains audit entries for 45 days, the server
owner must initialize an older channel if its create entry has expired. If the
channel is unbound, OpenAB uses the configured bot cookie to call
`POST /api/team-spaces` with the Discord channel name, then persists the
returned Space ID as the channel binding. The request sets `publicRead: true`,
allowing signed-in users to read the Space without membership. If a binding
already exists, no API request or file update is performed.

### `/export-thread`

Fetches the current Discord thread or DM history and returns a `.txt` file as an ephemeral follow-up. The transcript includes message timestamps, author names and IDs, message text, and attachment URLs.

**Optional parameters** (mutually exclusive — use at most one):

| Parameter | Type | Description |
|-----------|------|-------------|
| `limit` | Integer | Export only the most recent N messages (1–5000) |
| `since` | String | Export messages after this message ID (right-click → Copy Message ID) |
| `days` | Integer | Export messages from the last N days (1–365). Rolling N×24h window from now. |
| `all` | Boolean | Export all messages (up to 5000) |

If no parameter is provided, the **last 100 messages** are exported.

**Examples:**
```
/export-thread                              → last 100 messages (default)
/export-thread limit:500                    → most recent 500 messages
/export-thread since:1503744866100842698    → messages after this specific message
/export-thread days:3                       → messages from the last 3 days (rolling 72h)
/export-thread all:true                     → export all (cap 5000)
```

**Constraints:**
- Only works in allowed Discord threads or enabled DMs.
- Specifying more than one filter returns an error.
- Very large exports may be truncated to fit Discord's attachment size limit.

## Passing CLI Commands via @mention

In addition to slash commands, you can pass built-in CLI commands directly after an @mention:

```
@MyBot /compact
@MyBot /clear
@MyBot /model claude-sonnet-4
```

These are forwarded as-is to the ACP session as a prompt. Any command the underlying CLI supports in its interactive mode works here. This is the recommended workaround for agents that don't expose `configOptions`.

## `/remind`

Set a one-shot delayed reminder that mentions users or roles in the channel after a specified delay.

**Syntax:**
```
/remind targets:<@user @role ...> message:<text> delay:<duration>
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `targets` | Yes | Space-separated @mentions (users and/or roles) |
| `message` | Yes | Reminder text |
| `delay` | Yes | Duration before firing: `1m` to `30d` (supports `m`, `h`, `d` and combinations like `1h30m`) |

**Constraints:**
- Only humans can use `/remind` (bots are rejected)
- Minimum delay: 1 minute
- Maximum delay: 30 days
- Maximum message length: 1800 characters
- Maximum 5 active reminders per user
- Maximum 10 mention targets per reminder (use a @role for larger groups)
- `@everyone` and `@here` in messages are automatically neutralized (will not trigger mass mentions)
- One-shot only (fires once, then removed)
- Reminders persist across bot restarts (stored in `$HOME/.openab/reminders.json`)

**Examples:**
```
/remind targets:@Alice @Bob message:Review PR #42 delay:2h
/remind targets:@Reviewers message:Stand-up time delay:30m
/remind targets:@Charlie message:Check deployment delay:1d
```

**When fired, the bot posts:**
```
⏰ Reminder from @sender:
"Review PR #42"
cc @Alice @Bob
```
