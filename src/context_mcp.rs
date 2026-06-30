use crate::config::{resolve_allow_all, ContextMcpConfig, DiscordConfig, SlackConfig};
use anyhow::{anyhow, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{info, warn};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const TOOL_READ_CURRENT_THREAD: &str = "read_current_thread";
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const SLACK_API_BASE: &str = "https://slack.com/api";

type RespBody = Full<Bytes>;

#[derive(Clone)]
pub struct ContextMcpServer {
    bind: SocketAddr,
    state: Arc<ContextMcpState>,
}

#[derive(Clone)]
struct ContextMcpState {
    token: String,
    route_path: String,
    default_limit: usize,
    max_limit: usize,
    allowed_platforms: HashSet<String>,
    discord: Option<DiscordContext>,
    slack: Option<SlackContext>,
    discord_api_base: String,
    slack_api_base: String,
    http: reqwest::Client,
    #[cfg(test)]
    mock_read_result: Option<ThreadReadResult>,
}

#[derive(Clone)]
struct DiscordContext {
    token: String,
    allow_all_channels: bool,
    allowed_channels: HashSet<String>,
    allow_normal_channels: bool,
}

#[derive(Clone)]
struct SlackContext {
    token: String,
    allow_all_channels: bool,
    allowed_channels: HashSet<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct NormalizedAttachment {
    id: Option<String>,
    filename: Option<String>,
    url: Option<String>,
    content_type: Option<String>,
    size: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct NormalizedMessage {
    id: String,
    author_id: Option<String>,
    author_name: Option<String>,
    timestamp: Option<String>,
    text: String,
    attachments: Vec<NormalizedAttachment>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct ThreadReadResult {
    platform: String,
    channel_id: String,
    thread_id: Option<String>,
    messages: Vec<NormalizedMessage>,
    limit: usize,
}

#[derive(Debug)]
struct ReadRequest {
    platform: String,
    channel_id: String,
    thread_id: Option<String>,
    message_id: Option<String>,
    limit: Option<usize>,
}

impl ContextMcpServer {
    pub fn from_config(
        config: &ContextMcpConfig,
        discord: Option<&DiscordConfig>,
        slack: Option<&SlackConfig>,
    ) -> Result<Self> {
        let bind = config
            .bind
            .parse::<SocketAddr>()
            .map_err(|e| anyhow!("context_mcp.bind is not a valid socket address: {e}"))?;
        let allowed_platforms = config
            .allowed_platforms
            .iter()
            .map(|p| p.to_string())
            .collect::<HashSet<_>>();

        let discord = discord.map(|d| DiscordContext {
            token: d.bot_token.clone(),
            allow_all_channels: resolve_allow_all(d.allow_all_channels, &d.allowed_channels),
            allowed_channels: d.allowed_channels.iter().cloned().collect(),
            allow_normal_channels: config.allow_discord_normal_channels,
        });
        let slack = slack.map(|s| SlackContext {
            token: s.bot_token.clone(),
            allow_all_channels: resolve_allow_all(s.allow_all_channels, &s.allowed_channels),
            allowed_channels: s.allowed_channels.iter().cloned().collect(),
        });

        Ok(Self {
            bind,
            state: Arc::new(ContextMcpState {
                token: config.token.clone(),
                route_path: config.route_path.clone(),
                default_limit: config.default_limit,
                max_limit: config.max_limit,
                allowed_platforms,
                discord,
                slack,
                discord_api_base: DISCORD_API_BASE.into(),
                slack_api_base: SLACK_API_BASE.into(),
                http: reqwest::Client::new(),
                #[cfg(test)]
                mock_read_result: None,
            }),
        })
    }

    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        let listener = TcpListener::bind(self.bind).await?;
        info!(bind = %self.bind, "context MCP server listening");

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("context MCP server received shutdown signal");
                    return Ok(());
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let state = self.state.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req| {
                            let state = state.clone();
                            async move { Ok::<_, Infallible>(handle_http(req, state).await) }
                        });
                        if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                            warn!(error = %e, "context MCP connection failed");
                        }
                    });
                }
            }
        }
    }
}

async fn handle_http(req: Request<Incoming>, state: Arc<ContextMcpState>) -> Response<RespBody> {
    if req.method() != Method::POST {
        return text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    }
    if !route_matches(req.uri().path(), &state.route_path) {
        return text_response(StatusCode::NOT_FOUND, "not found");
    }
    if !authorized(&req, &state.token) {
        return text_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json_rpc_error(
                    Value::Null,
                    -32700,
                    &format!("failed to read request body: {e}"),
                ),
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(e) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json_rpc_error(Value::Null, -32700, &format!("invalid JSON: {e}")),
            );
        }
    };

    match handle_rpc(value, &state).await {
        RpcOutcome::Response(value) => json_response(StatusCode::OK, value),
        RpcOutcome::Accepted => text_response(StatusCode::ACCEPTED, ""),
    }
}

enum RpcOutcome {
    Response(Value),
    Accepted,
}

fn route_matches(request_path: &str, configured_path: &str) -> bool {
    request_path == configured_path || (configured_path == "/mcp" && request_path == "/")
}

async fn handle_rpc(value: Value, state: &ContextMcpState) -> RpcOutcome {
    let id = value.get("id").cloned();
    let Some(method) = value.get("method").and_then(|m| m.as_str()) else {
        return RpcOutcome::Response(json_rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "missing method",
        ));
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => RpcOutcome::Response(json_rpc_result(
            id.unwrap_or(Value::Null),
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "openab-context", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Use read_current_thread only when the user request depends on prior Discord or Slack thread context. Prefer small limits and current sender_context routing fields.",
            }),
        )),
        "notifications/initialized" => RpcOutcome::Accepted,
        "tools/list" => RpcOutcome::Response(json_rpc_result(
            id.unwrap_or(Value::Null),
            json!({"tools": [read_current_thread_tool()]}),
        )),
        "tools/call" => {
            let id = id.unwrap_or(Value::Null);
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name != TOOL_READ_CURRENT_THREAD {
                return RpcOutcome::Response(json_rpc_error(
                    id,
                    -32602,
                    &format!("unknown tool: {name}"),
                ));
            }
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match read_current_thread(args, state).await {
                Ok(result) => {
                    let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                    RpcOutcome::Response(json_rpc_result(
                        id,
                        json!({
                            "content": [{"type": "text", "text": text}],
                            "structuredContent": result,
                        }),
                    ))
                }
                Err(e) => RpcOutcome::Response(json_rpc_error(id, -32000, &e.to_string())),
            }
        }
        _ => RpcOutcome::Response(json_rpc_error(
            id.unwrap_or(Value::Null),
            -32601,
            &format!("method not found: {method}"),
        )),
    }
}

fn authorized<B>(req: &Request<B>, expected: &str) -> bool {
    let Some(header) = req.headers().get(AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = header.to_str() else {
        return false;
    };
    value
        .strip_prefix("Bearer ")
        .is_some_and(|token| token == expected)
}

fn read_current_thread_tool() -> Value {
    json!({
        "name": TOOL_READ_CURRENT_THREAD,
        "description": "Read bounded history from the current OpenAB Discord or Slack thread/DM. Use channel/thread ids from <sender_context>.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "platform": {
                    "type": "string",
                    "enum": ["discord", "slack"],
                    "description": "The sender_context channel/platform value."
                },
                "channel_id": {
                    "type": "string",
                    "description": "Discord parent/DM channel id or Slack channel id from sender_context."
                },
                "thread_id": {
                    "type": "string",
                    "description": "Discord thread channel id or Slack thread_ts from sender_context."
                },
                "message_id": {
                    "type": "string",
                    "description": "Optional current message id for future targeting; ignored by this tool."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum messages to return, clamped by OpenAB configuration."
                }
            },
            "required": ["platform", "channel_id"]
        }
    })
}

async fn read_current_thread(args: Value, state: &ContextMcpState) -> Result<ThreadReadResult> {
    let req = parse_read_request(&args)?;
    if !state.allowed_platforms.is_empty() && !state.allowed_platforms.contains(&req.platform) {
        anyhow::bail!("platform is not enabled for context MCP: {}", req.platform);
    }
    let limit = clamp_limit(req.limit, state.default_limit, state.max_limit);
    #[cfg(test)]
    if let Some(mut result) = state.mock_read_result.clone() {
        result.limit = limit;
        return Ok(result);
    }

    match req.platform.as_str() {
        "discord" => {
            let discord = state
                .discord
                .as_ref()
                .ok_or_else(|| anyhow!("Discord is not configured"))?;
            read_discord_thread(&state.http, &state.discord_api_base, discord, req, limit).await
        }
        "slack" => {
            let slack = state
                .slack
                .as_ref()
                .ok_or_else(|| anyhow!("Slack is not configured"))?;
            read_slack_thread(&state.http, &state.slack_api_base, slack, req, limit).await
        }
        other => anyhow::bail!("unsupported platform: {other}"),
    }
}

fn parse_read_request(args: &Value) -> Result<ReadRequest> {
    let platform = required_string(args, "platform")?;
    let channel_id = required_string(args, "channel_id")?;
    let thread_id = optional_string(args, "thread_id");
    let message_id = optional_string(args, "message_id");
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0);
    Ok(ReadRequest {
        platform,
        channel_id,
        thread_id,
        message_id,
        limit,
    })
}

fn required_string(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing required argument: {key}"))
}

fn optional_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn clamp_limit(requested: Option<usize>, default_limit: usize, max_limit: usize) -> usize {
    requested.unwrap_or(default_limit).min(max_limit).max(1)
}

async fn read_discord_thread(
    client: &reqwest::Client,
    api_base: &str,
    discord: &DiscordContext,
    req: ReadRequest,
    limit: usize,
) -> Result<ThreadReadResult> {
    let target_id = req
        .thread_id
        .clone()
        .unwrap_or_else(|| req.channel_id.clone());
    let channel = discord_get(
        client,
        api_base,
        &discord.token,
        &format!("/channels/{target_id}"),
    )
    .await?;
    let channel_type = channel.get("type").and_then(|v| v.as_u64()).unwrap_or(0);
    let parent_id = channel
        .get("parent_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let is_thread = matches!(channel_type, 10..=12);
    let is_dm = matches!(channel_type, 1 | 3);
    validate_discord_channel_scope(
        discord,
        &req,
        &target_id,
        is_thread,
        is_dm,
        parent_id.as_ref(),
    )?;

    let messages = discord_get(
        client,
        api_base,
        &discord.token,
        &format!("/channels/{target_id}/messages?limit={limit}"),
    )
    .await?;
    let mut messages = messages
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(normalize_discord_message)
        .collect::<Vec<_>>();
    messages.reverse();

    Ok(ThreadReadResult {
        platform: "discord".into(),
        channel_id: req.channel_id,
        thread_id: if is_thread { Some(target_id) } else { None },
        messages,
        limit,
    })
}

fn validate_discord_channel_scope(
    discord: &DiscordContext,
    req: &ReadRequest,
    target_id: &str,
    is_thread: bool,
    is_dm: bool,
    parent_id: Option<&String>,
) -> Result<()> {
    if !is_thread && !is_dm && !discord.allow_normal_channels {
        anyhow::bail!("Discord normal channel history is not available through this tool");
    }
    if !discord.allow_all_channels
        && !discord.allowed_channels.contains(&req.channel_id)
        && !discord.allowed_channels.contains(target_id)
        && !parent_id.is_some_and(|p| discord.allowed_channels.contains(p))
    {
        anyhow::bail!("Discord channel is not allowed for context reads");
    }
    Ok(())
}

async fn discord_get(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    path: &str,
) -> Result<Value> {
    let url = format!("{}{path}", api_base.trim_end_matches('/'));
    let resp = client
        .get(url)
        .header(AUTHORIZATION, format!("Bot {token}"))
        .send()
        .await?;
    let status = resp.status();
    let json: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = json
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Discord API error");
        anyhow::bail!("Discord API returned {status}: {detail}");
    }
    Ok(json)
}

fn normalize_discord_message(msg: Value) -> NormalizedMessage {
    let author = msg.get("author").unwrap_or(&Value::Null);
    let author_name = author
        .get("global_name")
        .or_else(|| author.get("username"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let attachments = msg
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|att| NormalizedAttachment {
                    id: att.get("id").and_then(|v| v.as_str()).map(str::to_string),
                    filename: att
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    url: att.get("url").and_then(|v| v.as_str()).map(str::to_string),
                    content_type: att
                        .get("content_type")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    size: att.get("size").and_then(|v| v.as_u64()),
                })
                .collect()
        })
        .unwrap_or_default();

    NormalizedMessage {
        id: msg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        author_id: author
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        author_name,
        timestamp: msg
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        text: msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        attachments,
    }
}

async fn read_slack_thread(
    client: &reqwest::Client,
    api_base: &str,
    slack: &SlackContext,
    req: ReadRequest,
    limit: usize,
) -> Result<ThreadReadResult> {
    if !slack.allow_all_channels && !slack.allowed_channels.contains(&req.channel_id) {
        anyhow::bail!("Slack channel is not allowed for context reads");
    }
    let thread_ts = req
        .thread_id
        .clone()
        .or(req.message_id.clone())
        .ok_or_else(|| anyhow!("Slack thread_id is required for context reads"))?;
    let resp = client
        .get(format!(
            "{}/conversations.replies",
            api_base.trim_end_matches('/')
        ))
        .header(AUTHORIZATION, format!("Bearer {}", slack.token))
        .query(&[
            ("channel", req.channel_id.as_str()),
            ("ts", thread_ts.as_str()),
            ("limit", &limit.to_string()),
            ("inclusive", "true"),
        ])
        .send()
        .await?;
    let json: Value = resp.json().await?;
    if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        anyhow::bail!("Slack API conversations.replies: {err}");
    }
    let messages = json
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().cloned().map(normalize_slack_message).collect())
        .unwrap_or_default();
    Ok(ThreadReadResult {
        platform: "slack".into(),
        channel_id: req.channel_id,
        thread_id: Some(thread_ts),
        messages,
        limit,
    })
}

fn normalize_slack_message(msg: Value) -> NormalizedMessage {
    let attachments = msg
        .get("files")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|file| NormalizedAttachment {
                    id: file.get("id").and_then(|v| v.as_str()).map(str::to_string),
                    filename: file
                        .get("name")
                        .or_else(|| file.get("title"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    url: file
                        .get("url_private_download")
                        .or_else(|| file.get("url_private"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    content_type: file
                        .get("mimetype")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    size: file.get("size").and_then(|v| v.as_u64()),
                })
                .collect()
        })
        .unwrap_or_default();

    NormalizedMessage {
        id: msg
            .get("ts")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        author_id: msg
            .get("user")
            .or_else(|| msg.get("bot_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        author_name: msg
            .get("username")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        timestamp: msg.get("ts").and_then(|v| v.as_str()).map(str::to_string),
        text: msg
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        attachments,
    }
}

fn json_rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn json_response(status: StatusCode, value: Value) -> Response<RespBody> {
    let mut response = Response::new(Full::new(Bytes::from(value.to_string())));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, "application/json".parse().unwrap());
    response
}

fn text_response(status: StatusCode, text: &str) -> Response<RespBody> {
    let mut response = Response::new(Full::new(Bytes::from(text.to_string())));
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> ContextMcpState {
        ContextMcpState {
            token: "secret".into(),
            route_path: "/mcp".into(),
            default_limit: 50,
            max_limit: 100,
            allowed_platforms: HashSet::new(),
            discord: None,
            slack: None,
            discord_api_base: DISCORD_API_BASE.into(),
            slack_api_base: SLACK_API_BASE.into(),
            http: reqwest::Client::new(),
            mock_read_result: None,
        }
    }

    #[test]
    fn clamp_limit_uses_default_and_max() {
        assert_eq!(clamp_limit(None, 50, 100), 50);
        assert_eq!(clamp_limit(Some(150), 50, 100), 100);
        assert_eq!(clamp_limit(Some(10), 50, 100), 10);
    }

    #[test]
    fn bearer_auth_requires_matching_token() {
        let missing = Request::builder().body(()).unwrap();
        assert!(!authorized(&missing, "secret"));

        let invalid = Request::builder()
            .header(AUTHORIZATION, "Bearer wrong")
            .body(())
            .unwrap();
        assert!(!authorized(&invalid, "secret"));

        let valid = Request::builder()
            .header(AUTHORIZATION, "Bearer secret")
            .body(())
            .unwrap();
        assert!(authorized(&valid, "secret"));
    }

    #[test]
    fn route_matching_uses_configured_path() {
        assert!(route_matches("/openab-codex/", "/openab-codex/"));
        assert!(!route_matches("/mcp", "/openab-codex/"));
        assert!(route_matches("/", "/mcp"));
    }

    #[test]
    fn discord_normal_channel_requires_explicit_opt_in() {
        let req = ReadRequest {
            platform: "discord".into(),
            channel_id: "C1".into(),
            thread_id: None,
            message_id: None,
            limit: None,
        };
        let mut allowed_channels = HashSet::new();
        allowed_channels.insert("C1".to_string());
        let discord = DiscordContext {
            token: "token".into(),
            allow_all_channels: false,
            allowed_channels,
            allow_normal_channels: false,
        };

        let err =
            validate_discord_channel_scope(&discord, &req, "C1", false, false, None).unwrap_err();
        assert!(err
            .to_string()
            .contains("Discord normal channel history is not available"));

        let discord = DiscordContext {
            allow_normal_channels: true,
            ..discord
        };
        validate_discord_channel_scope(&discord, &req, "C1", false, false, None).unwrap();
    }

    #[tokio::test]
    async fn tools_list_exposes_read_current_thread() {
        let state = test_state();
        let RpcOutcome::Response(value) = handle_rpc(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            &state,
        )
        .await
        else {
            panic!("expected JSON-RPC response");
        };
        assert_eq!(
            value["result"]["tools"][0]["name"],
            TOOL_READ_CURRENT_THREAD
        );
        assert_eq!(
            value["result"]["tools"][0]["inputSchema"]["required"][0],
            "platform"
        );
    }

    #[tokio::test]
    async fn tools_call_rejects_unknown_tool() {
        let state = test_state();
        let RpcOutcome::Response(value) = handle_rpc(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "unknown", "arguments": {}}
            }),
            &state,
        )
        .await
        else {
            panic!("expected JSON-RPC response");
        };
        assert_eq!(value["error"]["code"], -32602);
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    #[tokio::test]
    async fn tools_call_reads_slack_thread_and_clamps_limit() {
        let mut state = test_state();
        state.mock_read_result = Some(ThreadReadResult {
            platform: "slack".into(),
            channel_id: "C1".into(),
            thread_id: Some("171000.1".into()),
            messages: vec![NormalizedMessage {
                id: "171000.1".into(),
                author_id: Some("U1".into()),
                author_name: None,
                timestamp: Some("171000.1".into()),
                text: "root".into(),
                attachments: Vec::new(),
            }],
            limit: 0,
        });

        let RpcOutcome::Response(value) = handle_rpc(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": TOOL_READ_CURRENT_THREAD,
                    "arguments": {
                        "platform": "slack",
                        "channel_id": "C1",
                        "thread_id": "171000.1",
                        "limit": 500
                    }
                }
            }),
            &state,
        )
        .await
        else {
            panic!("expected JSON-RPC response");
        };
        assert_eq!(value["result"]["structuredContent"]["limit"], 100);
        assert_eq!(
            value["result"]["structuredContent"]["messages"][0]["text"],
            "root"
        );
    }

    #[test]
    fn parse_read_request_requires_platform_and_channel() {
        let parsed = parse_read_request(&json!({
            "platform": "discord",
            "channel_id": "C1",
            "thread_id": "T1",
            "limit": 25
        }))
        .unwrap();
        assert_eq!(parsed.platform, "discord");
        assert_eq!(parsed.channel_id, "C1");
        assert_eq!(parsed.thread_id.as_deref(), Some("T1"));
        assert_eq!(parsed.limit, Some(25));
        assert!(parse_read_request(&json!({"platform": "discord"})).is_err());
    }

    #[test]
    fn normalizes_discord_attachments_without_body_download() {
        let msg = normalize_discord_message(json!({
            "id": "123",
            "content": "hello",
            "timestamp": "2026-06-30T00:00:00.000000+00:00",
            "author": {"id": "U1", "username": "alice"},
            "attachments": [{
                "id": "A1",
                "filename": "demo.txt",
                "url": "https://cdn.discordapp.com/demo.txt",
                "content_type": "text/plain",
                "size": 12
            }]
        }));
        assert_eq!(msg.id, "123");
        assert_eq!(msg.author_name.as_deref(), Some("alice"));
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(msg.attachments[0].filename.as_deref(), Some("demo.txt"));
    }

    #[test]
    fn normalizes_slack_files_without_body_download() {
        let msg = normalize_slack_message(json!({
            "ts": "171000.1",
            "text": "hello",
            "user": "U1",
            "files": [{
                "id": "F1",
                "name": "demo.txt",
                "url_private_download": "https://files.slack.com/demo.txt",
                "mimetype": "text/plain",
                "size": 12
            }]
        }));
        assert_eq!(msg.id, "171000.1");
        assert_eq!(msg.author_id.as_deref(), Some("U1"));
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(
            msg.attachments[0].url.as_deref(),
            Some("https://files.slack.com/demo.txt")
        );
    }
}
