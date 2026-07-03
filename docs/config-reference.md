# Configuration Reference

OpenAB is configured via a TOML file (default: `config.toml`). Environment variables can be interpolated using `${VAR_NAME}` syntax.

At least one adapter section (`[discord]` or `[slack]`) is required.

## Loading Config

Specify the config source with `--config` / `-c`:

```bash
# Local file (default: config.toml when omitted)
openab run -c config.toml

# Remote URL via HTTPS (recommended)
openab run -c https://example.com/config.toml

# Remote URL via HTTP (warns — avoid in production; config contains secrets)
openab run -c http://internal.example.com/config.toml
```

Remote config is fetched via HTTP GET with a 10-second timeout and a 1 MiB response size limit. Environment variable expansion (`${VAR}`) works identically on both local and remote config content.

> **Security best practice:** Never hardcode secrets in remote config files. Use environment variable references like `bot_token = "${DISCORD_BOT_TOKEN}"` and inject the actual values via local environment variables or Kubernetes Secrets. For centralized secret management with rotation and audit, use `[secrets.refs]` with AWS Secrets Manager or an exec provider — see [secrets-management.md](secrets-management.md). OpenAB expands `${VAR}` identically for both local and remote config.

---

## `[discord]`

Discord adapter. Requires a Discord bot token.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `bot_token` | string | *required* | Discord bot token. Use `${DISCORD_BOT_TOKEN}` for env var. |
| `allow_all_channels` | bool \| omit | auto-detect | `true` = all channels; `false` = only `allowed_channels`. Omitted = inferred from list (non-empty → false, empty → true). |
| `allowed_channels` | string[] | `[]` | Channel IDs to allow. Only checked when `allow_all_channels` resolves to false. |
| `allow_all_users` | bool \| omit | auto-detect | `true` = any user; `false` = only `allowed_users`. Omitted = inferred from list. |
| `allowed_users` | string[] | `[]` | User IDs to allow. Only checked when `allow_all_users` resolves to false. |
| `allow_bot_messages` | string | `"off"` | `"off"` — ignore all bot messages. `"mentions"` — only process bot messages that @mention this bot. `"all"` — process all bot messages (capped by `max_bot_turns`). |
| `trusted_bot_ids` | string[] | `[]` | When non-empty, only these bot IDs pass the bot gate. Empty = any bot (mode permitting). **Admission override:** a trusted bot that @mentions this bot bypasses `allow_bot_messages` mode entirely (treated as human @mention, can pull bot into threads). |
| `allow_user_messages` | string | `"involved"` | `"involved"` — reply in threads bot has participated in without @mention; channel messages require @mention; DMs always process. `"mentions"` — always require @mention. `"multibot-mentions"` — like `"involved"`, but require @mention once another bot has posted in the thread. |
| `allow_dm` | bool | `false` | `true` = respond to Discord DMs; `false` = ignore DMs. `allowed_users` still applies in DMs. Each DM user consumes one session slot. |
| `max_bot_turns` | u32 | `100` | Max consecutive bot turns per thread before throttling (soft limit). Human message resets the counter. A compiled-in hard cap of 1000 consecutive bot messages is always enforced. |
| `message_processing_mode` | string | `"per-message"` | Message dispatch mode: `"per-message"` (each message = own turn), `"per-thread"` (all messages in thread share one buffer), or `"per-lane"` (each sender gets own buffer). See [Message Dispatch Modes](message-dispatch-modes.md). |
| `normal_channel_reply_mode` | string | `"thread"` | Normal guild-channel `@bot` behavior: `"thread"` creates or joins a Discord thread from the triggering message; `"inline"` replies in the current channel and references the triggering message. Threads and DMs are unchanged. |
| `max_buffered_messages` | u32 | `10` | Per-thread/lane mpsc channel capacity. Only applies to `per-thread` / `per-lane` modes. |
| `max_batch_tokens` | u32 | `24000` | Soft token cap per ACP turn. Only applies to `per-thread` / `per-lane` modes. |

---

## `[slack]`

Slack adapter using Socket Mode. Requires both a Bot User OAuth Token and an App-Level Token.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `bot_token` | string | *required* | Bot User OAuth Token (`xoxb-...`). |
| `app_token` | string | *required* | App-Level Token (`xapp-...`) for Socket Mode. |
| `allow_all_channels` | bool \| omit | auto-detect | Same behavior as Discord. |
| `allowed_channels` | string[] | `[]` | Slack channel IDs (e.g. `C0123456789`). |
| `allow_all_users` | bool \| omit | auto-detect | Same behavior as Discord. |
| `allowed_users` | string[] | `[]` | Slack user IDs (e.g. `U0123456789`). |
| `allow_bot_messages` | string | `"off"` | Same as Discord. |
| `trusted_bot_ids` | string[] | `[]` | Slack Bot User IDs (`U...`) or Bot IDs (`B...`). `U...` matching resolves event Bot IDs via Slack `bots.info`, so the bot token needs `users:read`. |
| `allow_user_messages` | string | `"involved"` | Same as Discord. |
| `max_bot_turns` | u32 | `100` | Same as Discord. |
| `message_processing_mode` | string | `"per-message"` | Same as Discord. See [Message Dispatch Modes](message-dispatch-modes.md). |
| `normal_channel_reply_mode` | string | `"thread"` | Normal-channel `@bot` behavior: `"thread"` replies in a Slack thread rooted at the triggering message; `"inline"` replies as a top-level message in the current channel. Existing Slack threads are unchanged. |
| `max_buffered_messages` | u32 | `10` | Same as Discord. |
| `max_batch_tokens` | u32 | `24000` | Same as Discord. |
| `assistant_mode` | bool | `true` | Use Slack AI-app APIs where supported: `assistant.threads.setStatus` for status indicators in Slack Assistant DM/app threads, and native content streaming via `chat.startStream`/`appendStream`/`stopStream` instead of the post+edit loop. Normal channels and ordinary Slack threads keep emoji-reaction status because Slack assistant thread status is not valid there. Native streaming is suppressed when another bot is present in the thread. Requires an AI-app Slack app with `assistant:write` — set to `false` for non-AI Slack apps. When native streaming is active, the `reply_to` output directive is bypassed — the streamed message is itself the in-thread reply. |

---

## `[context_mcp]`

Optional Streamable HTTP MCP server for scoped chat-context reads. It is disabled
by default and intended for deployments where the ACP agent, such as Codex via
`codex-acp`, runs outside the OpenAB process or pod.

```toml
[context_mcp]
enabled = true
bind = "0.0.0.0:18080"
route_path = "/${APP_NAME}/"
token = "${OPENAB_CONTEXT_MCP_TOKEN}"
default_limit = 50
max_limit = 100
allowed_platforms = ["discord", "slack"]
allow_normal_channels = false
handoff_enabled = false
handoff_token_ttl_secs = 300
handoff_parent_summary_enabled = false
handoff_parent_summary_max_chars = 400
inject_into_agent = false
agent_url = "http://openab-context-mcp:18080/mcp"
agent_server_name = "openab_context"
agent_allowed_tools = ["read_current_thread", "read_message", "handoff_to_thread"]
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Starts the context MCP HTTP listener when true. Existing deployments are unchanged unless this is explicitly enabled. |
| `bind` | string | `"127.0.0.1:18080"` | Socket address for the MCP listener. Use a pod-local or cluster-local address; do not expose this endpoint publicly. |
| `route_path` | string | `"/mcp"` | HTTP path that accepts MCP JSON-RPC requests. Must start with `/` and may include a trailing slash, for example `"/${APP_NAME}/"` after env expansion. |
| `token` | string | `""` | Bearer token required for every MCP request. Required when `enabled = true`; use an environment variable reference such as `${OPENAB_CONTEXT_MCP_TOKEN}`. |
| `default_limit` | usize | `50` | Number of messages returned by `read_current_thread` when the MCP client omits `limit`. Must be positive and less than or equal to `max_limit`. |
| `max_limit` | usize | `100` | Hard cap for messages returned by any context read. Must be positive. |
| `allowed_platforms` | string[] | `[]` | Optional platform allow list. Empty means all configured platforms; non-empty values must be `"discord"` or `"slack"`. |
| `allow_normal_channels` | bool | `false` | Allows `read_current_thread` to read Discord or Slack normal channel history when the requested channel is allowed by OpenAB configuration. Keep disabled unless the agent needs channel-level context. |
| `handoff_enabled` | bool | `false` | Enables `handoff_to_thread`, an MCP action tool that lets an inline normal-channel agent start a thread-backed child task under the original user message. Requires `enabled = true`. |
| `handoff_token_ttl_secs` | u64 | `300` | Lifetime for each single-use handoff token exposed in `<sender_context>`. |
| `handoff_parent_summary_enabled` | bool | `false` | When true, the initial handoff child thread turn is summarized by the original normal-channel agent/session and sent as a concise reply to the original normal-channel message. Requires `handoff_enabled = true`. |
| `handoff_parent_summary_max_chars` | usize | `400` | Target maximum characters requested in the parent-agent summary prompt. OpenAB does not hard-truncate the summary before sending, so URLs are not cut by the broker. Must be positive. |
| `inject_into_agent` | bool | `false` | Adds this OpenAB MCP server to ACP `session/new` and `session/load` `mcpServers` payloads. Requires `agent_url`. |
| `agent_url` | string | — | Agent-reachable URL for this MCP endpoint, for example a Kubernetes Service URL. Required when `inject_into_agent = true`. |
| `agent_server_name` | string | `"openab_context"` | Name used for the injected MCP server. |
| `agent_allowed_tools` | string[] | `["read_current_thread", "read_message"]` | Optional OpenAB MCP tool allow-list for injected MCP configuration. Known tools are `read_current_thread`, `read_message`, and `handoff_to_thread`. When handoff is enabled, OpenAB adds `handoff_to_thread` to the injected server allow-list if missing. |

The MCP server exposes:

- `read_current_thread`, which returns normalized message history for the current
  Discord thread/DM or Slack thread. When explicitly enabled, it can also read
  bounded Discord or Slack normal-channel history. Reads are bounded by the
  configured limits and still respect OpenAB's adapter channel allowlists.
- `read_message`, which returns one normalized Discord or Slack message by
  `channel_id` and `message_id`. For Discord reply/quote messages, OpenAB adds
  `referenced_channel_id` and `referenced_message_id` to `<sender_context>`; pass
  those values to `read_message` when the referenced message is outside the
  recent thread history window.
- `handoff_to_thread`, when `handoff_enabled = true`, which creates a
  thread-backed task from the current inline normal-channel message and returns
  after the child task is enqueued. The agent passes
  `<sender_context>.handoff_token`, a thread title, and the child task prompt.

When MCP injection is configured explicitly under `[agent]`, OpenAB passes the
configured servers to ACP sessions as a name-keyed `mcpServers` object:

```toml
[[agent.mcp_servers]]
name = "openab_context"
type = "http"
url = "http://openab-context-mcp:18080/mcp"
headers = { Authorization = "Bearer ${OPENAB_CONTEXT_MCP_TOKEN}" }
allowed_tools = ["read_current_thread", "read_message", "handoff_to_thread"]
```

Normal channel history is rejected by default; pass the current `thread_id` from
`<sender_context>` for thread reads. If `allow_normal_channels = true`,
normal-channel reads are allowed only for channels that pass the platform
adapter's channel allowlist. Slack normal-channel reads use
`conversations.history`.

**Security:**
- Do not pass Discord or Slack bot tokens to the agent for history reads. Inject
  only the MCP bearer token into the agent pod.
- Keep the MCP Service cluster-local, rotate the bearer token through your secret
  manager, and restart both OpenAB and the agent pod after rotation.
- Treat the tool output as chat history. Keep `default_limit` small and set
  `max_limit` to the largest history window you are comfortable exposing.

---

## `[gateway]`

Custom Gateway adapter for platforms like Telegram, LINE, Feishu/Lark, and Google Chat. Connects to the gateway via WebSocket.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `url` | string | *required* | WebSocket URL of the gateway (e.g. `ws://openab-gateway:8080/ws`). |
| `platform` | string | `"telegram"` | Platform name for session key namespacing (e.g. `"telegram"`, `"line"`, `"feishu"`, `"googlechat"`). |
| `token` | string | — | Shared token for WebSocket authentication (optional but recommended). |
| `bot_username` | string | — | Bot username for @mention gating in groups. |
| `allow_all_channels` | bool \| omit | auto-detect | `true` = all channels; `false` = only `allowed_channels`. Omitted = inferred from list (non-empty → false, empty → true). |
| `allowed_channels` | string[] | `[]` | Chat/group IDs to allow. Only checked when `allow_all_channels` resolves to false. |
| `allow_all_users` | bool \| omit | auto-detect | `true` = any user; `false` = only `allowed_users`. Omitted = inferred from list. |
| `allowed_users` | string[] | `[]` | User IDs to allow. Only checked when `allow_all_users` resolves to false. |
| `message_processing_mode` | string | `"per-message"` | Same as Discord. See [Message Dispatch Modes](message-dispatch-modes.md). |
| `max_buffered_messages` | u32 | `10` | Same as Discord. |
| `max_batch_tokens` | u32 | `24000` | Same as Discord. |

---

## `[agent]`

The AI agent subprocess that OpenAB spawns to handle messages via ACP.

> **This entire section is optional.** If omitted, `command` and `args` default from `$OPENAB_AGENT_COMMAND` (e.g. `"opencode acp"` — first token is command, rest are args). Each Docker image sets this env var so you typically don't need an `[agent]` block unless you want to override `env` or `args`.

**Resolution priority:** config `[agent].command`/`args` > `$OPENAB_AGENT_COMMAND` > `"openab-agent"`

> **Partial override rule:** Setting `command` without `args` resets args to `[]`. This prevents a custom command from inheriting the env var's args. To keep env-var args with a custom command, set both fields explicitly.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `transport` | string | `"stdio"` | ACP transport: `"stdio"` (spawn a local subprocess) or `"websocket"` (connect to an ACP WebSocket endpoint). |
| `command` | string | from `$OPENAB_AGENT_COMMAND` or `"openab-agent"` | Agent binary for `transport = "stdio"`; optional unless you want to override the image default. |
| `args` | string[] | from `$OPENAB_AGENT_COMMAND` or `[]` | CLI arguments. Defaults to env var args only when `command` is also defaulted. |
| `url` | string | — | WebSocket URL for `transport = "websocket"` (e.g. `ws://stdio-to-ws:3000`). Required for websocket transport. |
| `headers` | map | `{}` | Extra HTTP headers sent during the WebSocket handshake. Only used for websocket transport. |
| `working_dir` | string | `$HOME` | Working directory for the agent process. Optional — defaults to container's `$HOME`. |
| `per_session_working_dir` | bool | `false` | When `true`, OpenAB creates a stable per-session subdirectory under `working_dir` and uses that as the agent cwd. Discord threads become paths like `working_dir/discord_<thread_id>`. |
| `env` | map | `{}` | Extra environment variables (e.g. `{ OPENAI_API_KEY = "${OPENAI_API_KEY}" }`). |
| `inherit_env` | string[] | `[]` | Env var names to inherit from the OAB process (e.g. vars injected via K8s `envFrom`). Keys in `env` take precedence. |

> **Default inherited vars:** After `env_clear()`, the agent always receives `HOME`, `PATH`, and `USER` (on Windows: `USERPROFILE`, `USERNAME`, `PATH`, `SystemRoot`, `SystemDrive`). Use `inherit_env` to pass additional vars beyond this baseline.

## S3 Discarded File Offload

OpenAB can upload files that were not injected into the prompt, such as unsupported file types or files skipped by size limits. This is disabled by default; existing deployments keep the same discard behavior unless `[s3] enabled = true` is configured with the required fields.

```toml
[s3]
enabled = true
bucket = "openab-discarded-files"
region = "us-east-1"
directory = "discarded"
access_key_id = "${S3_ACCESS_KEY_ID}"
secret_access_key = "${S3_SECRET_ACCESS_KEY}"
# endpoint_url = "http://minio:9000"
# force_path_style = true
# session_token = "${S3_SESSION_TOKEN}"
```

For Aliyun OSS S3-compatible endpoints, use virtual-hosted style:

```toml
[s3]
enabled = true
bucket = "univer-cli-fc-east"
region = "us-east-1"
endpoint_url = "https://s3.oss-us-east-1.aliyuncs.com"
force_path_style = false
directory = "/slack-test/data/"
access_key_id = "${OSS_ACCESS_KEY_ID}"
secret_access_key = "${OSS_ACCESS_KEY_SECRET}"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Enables discarded-file uploads when the required fields are also valid. |
| `bucket` | string | — | Destination S3 bucket. Required when enabled. |
| `region` | string | — | AWS region or S3-compatible region. Required when enabled. |
| `endpoint_url` | string | — | Optional S3-compatible endpoint, such as MinIO or LocalStack. |
| `force_path_style` | bool | `true` | Use path-style bucket addressing (`endpoint/bucket/key`). Set to `false` for providers such as Aliyun OSS that require virtual-hosted style (`bucket.endpoint/key`). |
| `directory` | string | — | Root key prefix for discarded files. Required when enabled. Use nested paths such as `prod/openab/discarded` for environment or tenant isolation. |
| `access_key_id` | string | AWS SDK default chain | Optional access key for this S3 target. When omitted, OpenAB uses the AWS SDK default credential chain. |
| `secret_access_key` | string | AWS SDK default chain | Optional secret key for this S3 target. Used only when `access_key_id` is also set. |
| `session_token` | string | — | Optional session token for temporary credentials. |

Object keys are rooted at `directory`; when `agent.per_session_working_dir = true`, they also include the sanitized session directory. Prompt hints include `discarded-file-offload`, the logical working path under `agent.working_dir`, and the object key.

### Authentication

Each image sets `OPENAB_AGENT_AUTH_COMMAND` with the correct auth command. To authenticate any agent:

```bash
kubectl exec -it deployment/openab-<name> -- sh -c "$OPENAB_AGENT_AUTH_COMMAND"
```

This works for all agents regardless of backend — no need to remember the specific auth command.

### Agent examples

```toml
# Kiro CLI
[agent]
command = "kiro-cli"
args = ["acp", "--trust-all-tools"]
working_dir = "/home/agent"
# per_session_working_dir = true

# Claude Code
[agent]
command = "claude-agent-acp"
args = []
working_dir = "/home/node"
# Auth: kubectl exec -it deploy/openab-claude -- claude auth login
# Credentials persist in HOME PVC across restarts. See docs/claude-code.md.

# Codex
[agent]
command = "codex-acp"
working_dir = "/home/node"
env = { OPENAI_API_KEY = "${OPENAI_API_KEY}" }

# Gemini CLI
[agent]
command = "gemini"
args = ["--acp"]
working_dir = "/home/node"
env = { GEMINI_API_KEY = "${GEMINI_API_KEY}" }

# GitHub Copilot
[agent]
command = "copilot"
args = ["--acp", "--stdio"]
working_dir = "/home/node"

# opencode
[agent]
command = "opencode"
args = ["acp"]
working_dir = "/home/node"

# Pi Agent
[agent]
command = "pi-acp"
working_dir = "/home/node"

# Cursor Agent
[agent]
command = "cursor-agent"
args = ["acp", "--model", "auto", "--workspace", "/home/agent"]
working_dir = "/home/agent"

# Hermes Agent
[agent]
command = "hermes-acp"
working_dir = "/home/agent"

# Remote ACP endpoint over WebSocket
[agent]
transport = "websocket"
url = "ws://stdio-to-ws:3000"
headers = { Authorization = "Bearer ${ACP_TOKEN}" }
working_dir = "/home/agent"
```

---

## `[pool]`

Session pool settings for managing concurrent agent sessions.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max_sessions` | usize | `10` | Maximum number of concurrent agent sessions. When full, the oldest idle session is suspended (recoverable); if all sessions are busy, new requests are rejected. |
| `session_ttl_hours` | float | `4.0` | Session time-to-live in hours. Supports fractional values such as `0.5` for 30 minutes. Idle sessions are reclaimed after this period. The example config uses `24`. |

---

## `[hooks]`

Lifecycle hooks that run custom scripts at specific points during the container lifecycle. See [hooks.md](hooks.md) for full documentation and examples.

### `[hooks.pre_boot]`

Runs **before** agent pool creation. Use for bootstrapping files, syncing from S3, installing CLIs.

### `[hooks.pre_shutdown]`

Runs **after** pool shutdown on SIGTERM. Use for backing up state, syncing to S3.

Both hooks share the same fields:

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `script` | string | — | Absolute path to an executable script. |
| `inline` | string | — | Script content (written to temp file and executed). |
| `url` | string | — | Remote script URL (max 1 MiB). |
| `sha256` | string | — | Required with `url` — hex-encoded SHA-256 checksum. |
| `timeout_seconds` | u64 | `60` | Max wall-clock seconds before the script is killed. |
| `on_failure` | string | `"abort"` | `"abort"` exits openab; `"warn"` logs and continues. |

> Exactly one of `script`, `inline`, or `url` must be set. `script` must be an absolute path. `url` requires `sha256`.

```toml
[hooks.pre_boot]
inline = '''
#!/bin/sh
set -e
aws s3 sync "$BOOTSTRAP_URI" "$HOME/"
'''
timeout_seconds = 120
on_failure = "abort"

[hooks.pre_shutdown]
inline = '''
#!/bin/sh
aws s3 sync "$HOME/" "s3://$STATE_BUCKET/$TASK_FAMILY/" \
  --exclude "aws-cli/*" --quiet
'''
timeout_seconds = 30
on_failure = "warn"
```

---

## `[secrets]`

External secrets management. Secrets are resolved at boot time (after `pre_boot` hooks) and held in memory only — never written to disk. See [secrets-management.md](secrets-management.md) for full documentation.

### `[secrets.refs]`

Secret references. Each key maps to a provider URI. Resolved values are available as `${secrets.<key>}` in other config fields.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `<name>` | string | — | URI referencing an external secret. Supported schemes: `aws-sm://`, `exec://`. |

**URI formats:**
- `aws-sm://<secret-id>#<json-key>` — fetch from AWS Secrets Manager, extract JSON field
- `exec://<absolute-script-path> <key> <attribute>` — run script with two arguments, read stdout

### `[secrets.aws]`

AWS Secrets Manager provider configuration (optional).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `region` | string | auto | Override AWS region. Defaults to SDK credential chain (env/IMDS/IRSA). |
| `endpoint_url` | string | — | Override endpoint URL (for LocalStack or VPC endpoints). |

### `[secrets.exec]`

Exec provider configuration (optional).

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `timeout_seconds` | u64 | `10` | Max seconds per script invocation before kill. |

```toml
[secrets.refs]
discord_token = "aws-sm://openab/prod#discord_bot_token"
openai_key    = "aws-sm://openab/prod#openai_api_key"
github_pat    = "exec:///home/agent/.local/bin/get-secret.sh vault/openab github_pat"

[secrets.aws]
region = "ap-northeast-1"

[secrets.exec]
timeout_seconds = 15

[discord]
bot_token = "${secrets.discord_token}"
```

---

## `[reactions]`

Emoji reaction feedback on messages to show agent processing status.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Enable/disable reaction feedback. |
| `remove_after_reply` | bool | `false` | Remove the status reaction after the agent replies. |
| `tool_display` | string | `"full"` | How tool calls are rendered: `"full"` (complete title), `"compact"` (count summary, e.g. `✅ 3 · 🔧 1 tool(s)`), or `"none"` (hidden). |

### `[reactions.emojis]`

Customize the emoji for each processing stage.

| Key | Default | Description |
|-----|---------|-------------|
| `queued` | 👀 | Message received, queued for processing. |
| `thinking` | 🤔 | Agent is thinking / generating. |
| `tool` | 🔥 | Agent is calling a tool. |
| `coding` | 👨‍💻 | Agent is writing code. |
| `web` | ⚡ | Agent is doing web operations. |
| `done` | 🆗 | Agent finished successfully. |
| `error` | 😱 | Agent encountered an error. |

### `[reactions.timing]`

Fine-tune reaction timing behavior (milliseconds).

| Key | Default | Description |
|-----|---------|-------------|
| `debounce_ms` | `700` | Debounce interval before updating the reaction emoji. |
| `stall_soft_ms` | `10000` | Soft stall threshold — warn if no progress. |
| `stall_hard_ms` | `30000` | Hard stall threshold — consider the agent stuck. |
| `done_hold_ms` | `1500` | How long to show the done emoji before removing (if `remove_after_reply`). |
| `error_hold_ms` | `2500` | How long to show the error emoji before removing. |

---

## `[stt]`

Speech-to-text transcription for voice messages. Uses an OpenAI-compatible `/audio/transcriptions` endpoint.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Enable voice message transcription. |
| `api_key` | string | `""` | API key for the STT service. When empty and `base_url` contains `groq.com`, the `GROQ_API_KEY` environment variable is used automatically. For local servers, use `api_key = "not-needed"`. |
| `model` | string | `"whisper-large-v3-turbo"` | Model name to use for transcription. |
| `base_url` | string | `"https://api.groq.com/openai/v1"` | Base URL of the STT API. Any OpenAI-compatible `/audio/transcriptions` endpoint works. |
| `echo_transcript` | bool | `false` | When set to `true` and STT runs, post a `> 🎤 <transcript>` message to the thread before the agent reply so users can verify what was heard. Failures show `(transcription failed)` and add a ⚠️ reaction to the original message. |

---

## `[workspace]`

Workspace aliases for [Control Directives](adr/control-directives.md). Users specify `[[ws:@alias]]` in their first message to set the session's working directory.

```toml
[workspace.aliases]
openab = "~/projects/openab"
infra  = "~/projects/infra-cdk"
web    = "~/projects/frontend"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `aliases` | map | `{}` | Key-value map of alias name → path. Users reference with `@` prefix: `[[ws:@openab]]`. Paths starting with `~` expand to `$HOME`. All paths must be within the bot's home directory (security boundary). |

**Security:**
- Relative paths are rejected
- `~` expands to bot home (`$HOME`)
- Paths are canonicalized and must be within bot home subtree
- Symlink escapes are caught by canonicalization
- Target must be an existing directory (not a file)

---

## `[cron]`

Everything cron-related lives under `[cron]`.

```toml
[cron]
usercron_enabled = true                      # enable hot-reload (default: false)
usercron_path = "cronjob.toml"               # relative to $HOME/.openab/, or absolute

[[cron.jobs]]
enabled = true                               # optional, default: true
schedule = "0 9 * * 1-5"                    # cron expression (5-field POSIX)
channel = "123456789"                        # target channel/thread ID
message = "summarize yesterday's merged PRs" # message sent to agent
platform = "discord"                         # optional, default: "discord"
sender_name = "DailyOps"                     # optional, default: "openab-cron"
timezone = "America/New_York"                # optional, default: "UTC"
thread_id = ""                               # optional, post to existing thread

[[cron.jobs]]
schedule = "0 0 * * 0"
channel = "123456789"
message = "generate weekly status report"
platform = "discord"
timezone = "UTC"
```

### `[cron]` fields

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `usercron_enabled` | bool | `false` | Enable usercron hot-reload. Must be explicitly set to `true`. |
| `usercron_path` | string | — | Path to the external `cronjob.toml`. Relative paths resolve from `$HOME/.openab/`. |

### `[[cron.jobs]]` fields

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Set `false` to disable without removing the entry. |
| `schedule` | string | *required* | Cron expression (minute, hour, day-of-month, month, day-of-week). |
| `channel` | string | *required* | Target Discord channel/thread ID or Slack channel ID. |
| `message` | string | *required* | Message sent to the agent as a prompt. |
| `platform` | string | `"discord"` | Target platform (`"discord"` or `"slack"`). |
| `sender_name` | string | `"openab-cron"` | Sender attribution shown in the prompt context. |
| `timezone` | string | `"UTC"` | IANA timezone for schedule evaluation (e.g. `"America/New_York"`, `"Europe/Berlin"`). |
| `thread_id` | string | `""` | Optional thread ID to post into an existing thread. |

The external `cronjob.toml` uses `[[jobs]]` (same fields). See [Usercron docs](cronjob.md#usercron--hot-reload-with-cronjobtoml) for details.

### Usercron-only `[[jobs]]` fields

These fields are valid only in the external usercron file, for example `$HOME/.openab/cronjob.toml`. They are rejected in baseline `[[cron.jobs]]` because OpenAB only writes state back to the user-managed cron file.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `id` | string | *required with `disable_on_success`* | Stable job ID used when the scheduler writes `enabled = false` or `thread_id` back to `cronjob.toml`. |
| `disable_on_success` | string | — | Command to run before sending the scheduled prompt. |
| `disable_on_success_match` | string | *required with `disable_on_success`* | Marker that must appear in stdout or stderr, in addition to exit code `0`, before the job is considered complete. |
| `disable_on_success_timeout_secs` | integer | `60` | Timeout for the completion check command. |
| `disable_on_success_working_dir` | string | — | Working directory for the completion check command. |

Example:

```toml
[[jobs]]
id = "fix-unit-tests"
enabled = true
schedule = "*/10 * * * *"
channel = "123456789"
message = "Unit tests are still failing. Continue fixing them."
disable_on_success = "npm test && echo OPENAB_GOAL_SUCCESS"
disable_on_success_match = "OPENAB_GOAL_SUCCESS"
disable_on_success_timeout_secs = 120
disable_on_success_working_dir = "/workspace/my-project"
```

**Cron expression format:**

```
┌───────────── minute (0-59)
│ ┌───────────── hour (0-23)
│ │ ┌───────────── day of month (1-31)
│ │ │ ┌───────────── month (1-12)
│ │ │ │ ┌───────────── day of week (0-7, 0 and 7 = Sunday)
│ │ │ │ │
* * * * *
```

**Behaviors:**
- Scheduler evaluates expressions once per minute
- If a previous execution is still running, the next tick is skipped (no overlap)
- Failed executions are logged but do not block other jobs or chat traffic
- Stateless — no persistence needed, re-evaluated from config on restart

---

## Customizing via Helm

When deploying with the Helm chart (`charts/openab`), the `config.toml` is generated from `values.yaml`. Each agent is defined under the `agents` map:

```yaml
agents:
  kiro:
    command: kiro-cli
    args: ["acp", "--trust-all-tools"]
    discord:
      enabled: true
      allowedChannels: ["1234567890"]
      allowBotMessages: "mentions"
      trustedBotIds: ["9876543210"]
    pool:
      maxSessions: 10
      sessionTtlHours: 24
    reactions:
      enabled: true
    stt:
      enabled: true
      apiKey: "your-groq-key"
```

Key mapping (`values.yaml` → `config.toml`):

| Helm value | Config key |
|---|---|
| `agents.<name>.discord.allowedChannels` | `[discord] allowed_channels` |
| `agents.<name>.discord.allowBotMessages` | `[discord] allow_bot_messages` |
| `agents.<name>.discord.trustedBotIds` | `[discord] trusted_bot_ids` |
| `agents.<name>.discord.allowUserMessages` | `[discord] allow_user_messages` |
| `agents.<name>.discord.messageProcessingMode` | `[discord] message_processing_mode` |
| `agents.<name>.discord.normalChannelReplyMode` | `[discord] normal_channel_reply_mode` |
| `agents.<name>.discord.maxBufferedMessages` | `[discord] max_buffered_messages` |
| `agents.<name>.discord.maxBatchTokens` | `[discord] max_batch_tokens` |
| `agents.<name>.slack.normalChannelReplyMode` | `[slack] normal_channel_reply_mode` |
| `agents.<name>.slack.*` | `[slack] *` (same pattern) |
| `agents.<name>.pool.maxSessions` | `[pool] max_sessions` |
| `agents.<name>.pool.sessionTtlHours` | `[pool] session_ttl_hours` |
| `agents.<name>.workspace.aliases.<alias>` | `[workspace.aliases] <alias>` |
| `agents.<name>.reactions.enabled` | `[reactions] enabled` |
| `agents.<name>.reactions.toolDisplay` | `[reactions] tool_display` |
| `agents.<name>.stt.apiKey` | `[stt] api_key` |
| `agents.<name>.contextMcp.handoffEnabled` | `[context_mcp] handoff_enabled` |
| `agents.<name>.contextMcp.injectIntoAgent` | `[context_mcp] inject_into_agent` |
| `agents.<name>.contextMcp.agentUrl` | `[context_mcp] agent_url` |
| `agents.<name>.cronjobs[].enabled` | `[[cron.jobs]] enabled` |
| `agents.<name>.cronjobs[].schedule` | `[[cron.jobs]] schedule` |
| `agents.<name>.cronjobs[].channel` | `[[cron.jobs]] channel` |
| `agents.<name>.cronjobs[].message` | `[[cron.jobs]] message` |
| `agents.<name>.cronjobs[].platform` | `[[cron.jobs]] platform` |
| `agents.<name>.cronjobs[].senderName` | `[[cron.jobs]] sender_name` |
| `agents.<name>.cronjobs[].timezone` | `[[cron.jobs]] timezone` |
| `agents.<name>.cronjobs[].threadId` | `[[cron.jobs]] thread_id` |

> ⚠️ Use `--set-string` (not `--set`) for Discord/Slack IDs to avoid float64 precision loss:
> ```bash
> helm upgrade --install mybot charts/openab \
>   --set-string agents.kiro.discord.allowedChannels[0]="1234567890"
> ```

See `charts/openab/values.yaml` for the full list of Helm values including `persistence`, `image`, `resources`, and multi-agent examples.

---

## Environment variable interpolation

Any value can reference environment variables with `${VAR_NAME}`:

```toml
bot_token = "${DISCORD_BOT_TOKEN}"
```

Undefined variables resolve to an empty string.
