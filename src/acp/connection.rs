use crate::acp::protocol::{
    parse_config_options, ConfigOption, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse,
};
use crate::config::{AgentConfig, AgentMcpServerConfig, AgentTransport};
use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Map;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, trace, warn};

/// Pick the most permissive selectable permission option from ACP options.
fn pick_best_option(options: &[Value]) -> Option<String> {
    let mut fallback: Option<&Value> = None;

    for kind in ["allow_always", "allow_once"] {
        if let Some(option) = options
            .iter()
            .find(|option| option.get("kind").and_then(|k| k.as_str()) == Some(kind))
        {
            return option
                .get("optionId")
                .and_then(|id| id.as_str())
                .map(str::to_owned);
        }
    }

    for option in options {
        let kind = option.get("kind").and_then(|k| k.as_str());
        if kind == Some("reject_once") || kind == Some("reject_always") {
            continue;
        }
        fallback = Some(option);
        break;
    }

    fallback
        .and_then(|option| option.get("optionId"))
        .and_then(|id| id.as_str())
        .map(str::to_owned)
}

/// Build a spec-compliant permission response with backward-compatible fallback.
fn build_permission_response(params: Option<&Value>) -> Value {
    match params
        .and_then(|p| p.get("options"))
        .and_then(|options| options.as_array())
    {
        None => json!({
            "outcome": {
                "outcome": "selected",
                "optionId": "allow_always"
            }
        }),
        Some(options) => {
            if let Some(option_id) = pick_best_option(options) {
                json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id
                    }
                })
            } else {
                json!({
                    "outcome": {
                        "outcome": "cancelled"
                    }
                })
            }
        }
    }
}

fn expand_env(val: &str) -> String {
    if val.starts_with("${") && val.ends_with('}') {
        let key = &val[2..val.len() - 1];
        std::env::var(key).unwrap_or_default()
    } else {
        val.to_string()
    }
}
use tokio::time::Instant;

/// A content block for the ACP prompt — either text or image.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text { text: String },
    Image { media_type: String, data: String },
}

impl ContentBlock {
    pub fn to_json(&self) -> Value {
        match self {
            ContentBlock::Text { text } => json!({
                "type": "text",
                "text": text
            }),
            ContentBlock::Image { media_type, data } => json!({
                "type": "image",
                "data": data,
                "mimeType": media_type
            }),
        }
    }
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriter = futures_util::stream::SplitSink<WsStream, Message>;
type WsReader = futures_util::stream::SplitStream<WsStream>;

#[derive(Clone)]
pub enum AcpWriter {
    Stdio(Arc<Mutex<ChildStdin>>),
    WebSocket(Arc<Mutex<WsWriter>>),
}

impl AcpWriter {
    pub async fn send_line(&self, data: &str) -> Result<()> {
        match self {
            Self::Stdio(stdin) => {
                let mut w = stdin.lock().await;
                w.write_all(data.as_bytes()).await?;
                w.write_all(b"\n").await?;
                w.flush().await?;
            }
            Self::WebSocket(ws) => {
                let mut w = ws.lock().await;
                w.send(Message::Text(format!("{data}\n"))).await?;
            }
        }
        Ok(())
    }

    pub async fn close(&self) {
        if let Self::WebSocket(ws) = self {
            let mut w = ws.lock().await;
            let _ = w.send(Message::Close(None)).await;
        }
    }
}

pub struct AcpConnection {
    _proc: Option<Child>,
    /// PID of the direct child, used as the process group ID for cleanup.
    child_pgid: Option<i32>,
    writer: AcpWriter,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
    pub acp_session_id: Option<String>,
    pub supports_load_session: bool,
    pub config_options: Vec<ConfigOption>,
    pub last_active: Instant,
    pub session_reset: bool,
    mcp_servers: Value,
    session_params: Map<String, Value>,
    session_context: Option<Value>,
    _reader_handle: JoinHandle<()>,
    _stderr_handle: Option<JoinHandle<()>>,
}

/// Build the final set of env vars for the agent subprocess.
/// `explicit` ([agent].env) takes precedence over `inherit` ([agent].inherit_env).
/// Returns (merged env map, list of keys that were inherited from the process).
fn build_agent_env(
    explicit: &std::collections::HashMap<String, String>,
    inherit_keys: &[String],
) -> (std::collections::HashMap<String, String>, Vec<String>) {
    let mut result: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut inherited: Vec<String> = Vec::new();

    for (k, v) in explicit {
        result.insert(k.clone(), expand_env(v));
    }

    for key in inherit_keys {
        if !result.contains_key(key) {
            if let Ok(v) = std::env::var(key) {
                result.insert(key.clone(), v);
                inherited.push(key.clone());
            }
        }
    }

    (result, inherited)
}

fn websocket_client_id(thread_key: &str) -> &str {
    thread_key.rsplit(':').next().unwrap_or(thread_key)
}

fn mcp_servers_json(servers: &[AgentMcpServerConfig]) -> Value {
    let mut items = Vec::new();
    for server in servers {
        let mut cfg = serde_json::Map::new();
        cfg.insert("name".into(), Value::String(server.name.clone()));
        cfg.insert("type".into(), Value::String(server.transport.clone()));
        cfg.insert("url".into(), Value::String(server.url.clone()));
        if !server.headers.is_empty() {
            cfg.insert("headers".into(), json!(server.headers));
        }
        if !server.allowed_tools.is_empty() {
            cfg.insert(
                "tool_filter".into(),
                json!({ "allow": server.allowed_tools }),
            );
            cfg.insert("allowedTools".into(), json!(server.allowed_tools));
        }
        items.push(Value::Object(cfg));
    }
    Value::Array(items)
}

fn build_session_params(
    extra: &Map<String, Value>,
    session_context: Option<&Value>,
    cwd: &str,
    session_id: Option<&str>,
    mcp_servers: &Value,
) -> Value {
    let mut params = extra.clone();
    if let Some(session_context) = session_context {
        params.insert("openabSession".into(), session_context.clone());
    }
    params.insert("cwd".into(), Value::String(cwd.to_string()));
    params.insert("mcpServers".into(), mcp_servers.clone());
    if let Some(session_id) = session_id {
        params.insert("sessionId".into(), Value::String(session_id.to_string()));
    }
    Value::Object(params)
}

async fn finish_reader_loop(
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
) {
    let mut map = pending.lock().await;
    for (_, tx) in map.drain() {
        let _ = tx.send(JsonRpcMessage {
            id: None,
            method: None,
            result: None,
            error: Some(crate::acp::protocol::JsonRpcError {
                code: -1,
                message: "connection closed".into(),
                data: None,
            }),
            params: None,
        });
    }
    let mut sub = notify_tx.lock().await;
    *sub = None;
}

async fn handle_incoming_message(
    msg: JsonRpcMessage,
    writer: &AcpWriter,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: &Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
) {
    if msg.method.as_deref() == Some("session/request_permission") {
        if let Some(id) = msg.id {
            let title = msg
                .params
                .as_ref()
                .and_then(|p| p.get("toolCall"))
                .and_then(|t| t.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("?");

            let outcome = build_permission_response(msg.params.as_ref());
            info!(title, %outcome, "auto-respond permission");
            let reply = JsonRpcResponse::new(id, outcome);
            if let Ok(data) = serde_json::to_string(&reply) {
                let _ = writer.send_line(&data).await;
            }
        }
        return;
    }

    if let Some(id) = msg.id {
        let mut map = pending.lock().await;
        if let Some(tx) = map.remove(&id) {
            let sub = notify_tx.lock().await;
            if let Some(ntx) = sub.as_ref() {
                let _ = ntx.send(JsonRpcMessage {
                    id: Some(id),
                    method: None,
                    result: msg.result.clone(),
                    error: msg.error.clone(),
                    params: None,
                });
            }
            let _ = tx.send(msg);
            return;
        }
        trace!(request_id = id, "stale id-bearing message after abandon");
    }

    let sub = notify_tx.lock().await;
    if let Some(tx) = sub.as_ref() {
        let _ = tx.send(msg);
    }
}

async fn handle_ws_value(
    value: Value,
    writer: &AcpWriter,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: &Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
) {
    if value.get("type").and_then(|v| v.as_str()) == Some("connected") {
        debug!(payload = %value, "ignoring stdio-to-ws connected envelope");
        return;
    }
    if value.get("type").and_then(|v| v.as_str()) == Some("reconnect") {
        debug!(payload = %value, "ignoring stdio-to-ws reconnect envelope");
        return;
    }

    match serde_json::from_value::<JsonRpcMessage>(value.clone()) {
        Ok(msg) => {
            debug!(payload = %value, "acp_recv");
            handle_incoming_message(msg, writer, pending, notify_tx).await;
        }
        Err(e) => {
            warn!(payload = %value, error = %e, "ignoring non-ACP websocket payload");
        }
    }
}

/// Reader loop body: reads JSON-RPC messages from `reader`, auto-replies
/// `session/request_permission` via `writer`, resolves pending responses,
/// and forwards notifications + stale id-bearing messages to the active
/// subscriber. Extracted as a free generic function so unit tests can drive
/// it with `tokio::io::duplex()` halves instead of a real child process.
pub(crate) async fn run_reader_loop<R, W>(
    reader: R,
    writer: Arc<Mutex<W>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                error!("reader error: {e}");
                break;
            }
        }
        let msg: JsonRpcMessage = match serde_json::from_str(line.trim()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        debug!(line = line.trim(), "acp_recv");

        if msg.method.as_deref() == Some("session/request_permission") {
            if let Some(id) = msg.id {
                let title = msg
                    .params
                    .as_ref()
                    .and_then(|p| p.get("toolCall"))
                    .and_then(|t| t.get("title"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("?");

                let outcome = build_permission_response(msg.params.as_ref());
                info!(title, %outcome, "auto-respond permission");
                let reply = JsonRpcResponse::new(id, outcome);
                if let Ok(data) = serde_json::to_string(&reply) {
                    let mut w = writer.lock().await;
                    let _ = w.write_all(format!("{data}\n").as_bytes()).await;
                    let _ = w.flush().await;
                }
            }
            continue;
        }

        if let Some(id) = msg.id {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(&id) {
                let sub = notify_tx.lock().await;
                if let Some(ntx) = sub.as_ref() {
                    let _ = ntx.send(JsonRpcMessage {
                        id: Some(id),
                        method: None,
                        result: msg.result.clone(),
                        error: msg.error.clone(),
                        params: None,
                    });
                }
                let _ = tx.send(msg);
                continue;
            }
            trace!(request_id = id, "stale id-bearing message after abandon");
        }

        let sub = notify_tx.lock().await;
        if let Some(tx) = sub.as_ref() {
            let _ = tx.send(msg);
        }
    }

    finish_reader_loop(pending, notify_tx).await;
}

async fn run_ws_reader_loop(
    mut reader: WsReader,
    writer: AcpWriter,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>>,
    notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>>,
) {
    let mut buf = String::new();

    while let Some(frame) = reader.next().await {
        let chunk = match frame {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Frame(_)) => continue,
            Err(e) => {
                error!("websocket reader error: {e}");
                break;
            }
        };

        buf.push_str(&chunk);
        loop {
            let mut stream = serde_json::Deserializer::from_str(&buf).into_iter::<Value>();
            let mut consumed = 0usize;
            let mut progressed = false;

            while let Some(item) = stream.next() {
                match item {
                    Ok(value) => {
                        consumed = stream.byte_offset();
                        progressed = true;
                        handle_ws_value(value, &writer, &pending, &notify_tx).await;
                    }
                    Err(e) if e.is_eof() => break,
                    Err(e) => {
                        consumed = stream.byte_offset();
                        warn!(buffer = %buf, error = %e, "dropping malformed websocket payload");
                        if consumed == 0 {
                            buf.clear();
                        }
                        break;
                    }
                }
            }

            if consumed > 0 {
                buf.drain(..consumed);
            }

            if !progressed {
                break;
            }
        }
    }

    let trailing = buf.trim();
    if !trailing.is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(trailing) {
            handle_ws_value(value, &writer, &pending, &notify_tx).await;
        }
    }

    finish_reader_loop(pending, notify_tx).await;
}

impl AcpConnection {
    pub async fn spawn(config: &AgentConfig, working_dir: &str, thread_key: &str) -> Result<Self> {
        let mcp_servers = mcp_servers_json(&config.mcp_servers);
        let session_params = config
            .session_params
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map<String, Value>>();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(None));

        match config.transport {
            AgentTransport::Stdio => {
                let command = config.command.as_str();
                let args = &config.args;
                let env = &config.env;
                let inherit_env = &config.inherit_env;
                info!(
                    transport = "stdio",
                    cmd = command,
                    ?args,
                    cwd = working_dir,
                    "spawning agent"
                );

                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .current_dir(working_dir);
                // Create a new process group so we can kill the entire tree.
                // SAFETY: setpgid is async-signal-safe (POSIX.1-2008) and called
                // before exec. Return value checked — failure means the child won't
                // have its own process group, so kill(-pgid) would be unsafe.
                #[cfg(unix)]
                unsafe {
                    cmd.pre_exec(|| {
                        if libc::setpgid(0, 0) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
                #[cfg(windows)]
                {
                    cmd.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
                }
                // Clear inherited env to prevent credential leakage (e.g. DISCORD_BOT_TOKEN).
                // Only [agent].env values + essential baseline vars are passed through.
                cmd.env_clear();
                // Preserve the real HOME so agents can find OAuth/auth files (~/.codex,
                // ~/.claude, ~/.config/gh, etc.). working_dir is already set via
                // current_dir() above and is not necessarily the user's home directory.
                cmd.env(
                    "HOME",
                    std::env::var("HOME").unwrap_or_else(|_| working_dir.into()),
                );
                cmd.env(
                    "PATH",
                    std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
                );
                #[cfg(unix)]
                {
                    cmd.env(
                        "USER",
                        std::env::var("USER").unwrap_or_else(|_| "agent".into()),
                    );
                }
                #[cfg(windows)]
                {
                    // Windows requires SystemRoot for DLL loading and basic OS functionality.
                    // USERPROFILE is the Windows equivalent of HOME.
                    cmd.env(
                        "USERPROFILE",
                        std::env::var("USERPROFILE").unwrap_or_else(|_| working_dir.into()),
                    );
                    cmd.env(
                        "USERNAME",
                        std::env::var("USERNAME").unwrap_or_else(|_| "agent".into()),
                    );
                    if let Ok(v) = std::env::var("SystemRoot") {
                        cmd.env("SystemRoot", v);
                    }
                    if let Ok(v) = std::env::var("SystemDrive") {
                        cmd.env("SystemDrive", v);
                    }
                }
                for (k, v) in env {
                    cmd.env(k, expand_env(v));
                }
                // Inherit selected env vars from the OAB process (e.g. vars injected
                // via Kubernetes envFrom).  Keys already in [agent].env are skipped —
                // explicit values take precedence.
                let (agent_env, inherited_keys) = build_agent_env(env, inherit_env);
                for (k, v) in &agent_env {
                    cmd.env(k, v);
                }
                if !agent_env.is_empty() {
                    let explicit_keys: Vec<&String> = env.keys().collect();
                    tracing::warn!(
                ?explicit_keys,
                ?inherited_keys,
                "[agent].env/inherit_env is set -- these values are accessible to the agent and could be exfiltrated via prompt injection"
            );
                }
                let mut proc = cmd
                    .spawn()
                    .map_err(|e| anyhow!("failed to spawn {command}: {e}"))?;
                let child_pgid = proc.id().and_then(|pid| i32::try_from(pid).ok());

                let stdout = proc.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
                let stdin = proc.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
                let stdin = Arc::new(Mutex::new(stdin));

                // Capture agent stderr and log it (ACP spec: agents MAY write to stderr
                // for logging; clients MAY capture or ignore it).
                let stderr_handle = if let Some(stderr) = proc.stderr.take() {
                    let cmd_name = command.to_string();
                    Some(tokio::spawn(async move {
                        let mut reader = BufReader::new(stderr);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            match reader.read_line(&mut line).await {
                                Ok(0) => break,
                                Ok(_) => {
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() {
                                        let sanitized: String = trimmed
                                            .chars()
                                            .filter(|c| !c.is_control() || *c == '\t')
                                            .collect();
                                        if !sanitized.is_empty() {
                                            tracing::warn!(agent = %cmd_name, "{sanitized}");
                                        }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }))
                } else {
                    None
                };

                let reader_handle = tokio::spawn(run_reader_loop(
                    stdout,
                    stdin.clone(),
                    pending.clone(),
                    notify_tx.clone(),
                ));

                Ok(Self {
                    _proc: Some(proc),
                    child_pgid,
                    writer: AcpWriter::Stdio(stdin),
                    next_id: AtomicU64::new(1),
                    pending,
                    notify_tx,
                    acp_session_id: None,
                    supports_load_session: false,
                    config_options: Vec::new(),
                    last_active: Instant::now(),
                    session_reset: false,
                    mcp_servers,
                    session_params: session_params.clone(),
                    session_context: None,
                    _reader_handle: reader_handle,
                    _stderr_handle: stderr_handle,
                })
            }
            AgentTransport::WebSocket => {
                let url = config
                    .url
                    .as_deref()
                    .ok_or_else(|| anyhow!("agent.url is required for websocket transport"))?;
                info!(
                    transport = "websocket",
                    url,
                    cwd = working_dir,
                    "connecting to agent"
                );

                let mut request = url.into_client_request()?;
                request.headers_mut().insert(
                    HeaderName::from_static("x-client-id"),
                    HeaderValue::from_str(websocket_client_id(thread_key))?,
                );
                for (key, value) in &config.headers {
                    let name = HeaderName::from_bytes(key.as_bytes())?;
                    let value = HeaderValue::from_str(value)?;
                    request.headers_mut().insert(name, value);
                }

                let (stream, _response) = connect_async(request)
                    .await
                    .map_err(|e| anyhow!("failed to connect websocket agent {url}: {e}"))?;
                let (ws_writer, ws_reader) = stream.split();
                let writer = AcpWriter::WebSocket(Arc::new(Mutex::new(ws_writer)));
                let reader_handle = tokio::spawn(run_ws_reader_loop(
                    ws_reader,
                    writer.clone(),
                    pending.clone(),
                    notify_tx.clone(),
                ));

                Ok(Self {
                    _proc: None,
                    child_pgid: None,
                    writer,
                    next_id: AtomicU64::new(1),
                    pending,
                    notify_tx,
                    acp_session_id: None,
                    supports_load_session: false,
                    config_options: Vec::new(),
                    last_active: Instant::now(),
                    session_reset: false,
                    mcp_servers,
                    session_params,
                    session_context: None,
                    _reader_handle: reader_handle,
                    _stderr_handle: None,
                })
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn set_session_context(&mut self, context: Value) {
        self.session_context = Some(context);
    }

    pub(crate) async fn send_raw(&self, data: &str) -> Result<()> {
        debug!(data = data.trim(), "acp_send");
        self.writer.send_line(data).await
    }

    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<JsonRpcMessage> {
        let id = self.next_id();
        let req = JsonRpcRequest::new(id, method, params);
        let data = serde_json::to_string(&req)?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        self.send_raw(&data).await?;

        let timeout_secs = if method == "session/new" { 120 } else { 30 };
        let resp = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx)
            .await
            .map_err(|_| anyhow!("timeout waiting for {method} response"))?
            .map_err(|_| anyhow!("channel closed waiting for {method}"))?;

        if let Some(err) = &resp.error {
            return Err(anyhow!("{err}"));
        }
        Ok(resp)
    }

    pub async fn initialize(&mut self) -> Result<()> {
        let resp = self
            .send_request(
                "initialize",
                Some(json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {"name": "openab", "version": "0.1.0"},
                })),
            )
            .await?;

        let result = resp.result.as_ref();
        let agent_name = result
            .and_then(|r| r.get("agentInfo"))
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown");
        self.supports_load_session = result
            .and_then(|r| r.get("agentCapabilities"))
            .and_then(|c| c.get("loadSession"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        info!(
            agent = agent_name,
            load_session = self.supports_load_session,
            "initialized"
        );
        Ok(())
    }

    pub async fn session_new(&mut self, cwd: &str) -> Result<String> {
        let resp = self
            .send_request(
                "session/new",
                Some(build_session_params(
                    &self.session_params,
                    self.session_context.as_ref(),
                    cwd,
                    None,
                    &self.mcp_servers,
                )),
            )
            .await?;

        let session_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("sessionId"))
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("no sessionId in session/new response"))?
            .to_string();

        info!(session_id = %session_id, "session created");
        self.acp_session_id = Some(session_id.clone());
        if let Some(result) = resp.result.as_ref() {
            self.config_options = parse_config_options(result);
            if !self.config_options.is_empty() {
                info!(count = self.config_options.len(), "parsed configOptions");
            }
        }
        Ok(session_id)
    }

    /// Set a config option (e.g. model, mode) via ACP session/set_config_option.
    /// Returns the updated list of all config options.
    pub async fn set_config_option(
        &mut self,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<ConfigOption>> {
        let session_id = self
            .acp_session_id
            .as_ref()
            .ok_or_else(|| anyhow!("no session"))?
            .clone();

        let resp = self
            .send_request(
                "session/set_config_option",
                Some(json!({
                    "sessionId": session_id,
                    "configId": config_id,
                    "value": value,
                })),
            )
            .await;

        match resp {
            Ok(r) => {
                if let Some(result) = r.result.as_ref() {
                    self.config_options = parse_config_options(result);
                }
                info!(config_id, value, "config option set");
            }
            Err(_) => {
                // Fall back: send as a slash command (e.g. "/model claude-sonnet-4")
                let cmd = format!("/{config_id} {value}");
                info!(
                    cmd,
                    "set_config_option not supported, falling back to prompt"
                );
                let _resp = self
                    .send_request(
                        "session/prompt",
                        Some(json!({
                            "sessionId": session_id,
                            "prompt": [{"type": "text", "text": cmd}],
                        })),
                    )
                    .await?;
                for opt in &mut self.config_options {
                    if opt.id == config_id {
                        opt.current_value = value.to_string();
                    }
                }
            }
        }

        Ok(self.config_options.clone())
    }

    /// Send a prompt with content blocks (text and/or images) and return a receiver
    /// for streaming notifications. The final message on the channel will have id set
    /// (the prompt response).
    pub async fn session_prompt(
        &mut self,
        content_blocks: Vec<ContentBlock>,
    ) -> Result<(mpsc::UnboundedReceiver<JsonRpcMessage>, u64)> {
        self.last_active = Instant::now();

        let session_id = self
            .acp_session_id
            .as_ref()
            .ok_or_else(|| anyhow!("no session"))?;

        let (tx, rx) = mpsc::unbounded_channel();
        *self.notify_tx.lock().await = Some(tx);

        let id = self.next_id();

        // Convert content blocks to JSON
        let prompt_json: Vec<Value> = content_blocks.iter().map(|b| b.to_json()).collect();

        let req = JsonRpcRequest::new(
            id,
            "session/prompt",
            Some(json!({
                "sessionId": session_id,
                "prompt": prompt_json,
            })),
        );
        let data = serde_json::to_string(&req)?;

        let (resp_tx, _resp_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, resp_tx);

        self.send_raw(&data).await?;
        Ok((rx, id))
    }

    /// Call after prompt streaming is done to clean up subscriber.
    pub async fn prompt_done(&mut self) {
        *self.notify_tx.lock().await = None;
        self.last_active = Instant::now();
    }

    /// Drop the pending entry for `request_id` and best-effort send
    /// `session/cancel` as a JSON-RPC notification (no id; per ACP spec the
    /// agent does not reply). Errors are swallowed: the transport may
    /// already be dead, in which case the write fails harmlessly.
    /// See #732.
    pub async fn abandon_request(&self, request_id: u64) {
        self.pending.lock().await.remove(&request_id);
        let Some(session_id) = self.acp_session_id.as_deref() else {
            return;
        };
        let req = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {"sessionId": session_id},
        });
        if let Ok(data) = serde_json::to_string(&req) {
            let _ = self.send_raw(&data).await;
        }
    }

    /// Return a clone of the writer handle for lock-free cancel.
    pub fn cancel_handle(&self) -> AcpWriter {
        self.writer.clone()
    }

    pub fn alive(&self) -> bool {
        !self._reader_handle.is_finished()
    }

    pub async fn close_transport(&mut self) {
        self.writer.close().await;
        self._reader_handle.abort();
        if let Some(handle) = self._stderr_handle.take() {
            handle.abort();
        }
        finish_reader_loop(self.pending.clone(), self.notify_tx.clone()).await;
    }

    /// Resume a previous session by ID. Returns Ok(()) if the agent accepted
    /// the load, or an error if it failed (caller should fall back to session/new).
    pub async fn session_load(&mut self, session_id: &str, cwd: &str) -> Result<()> {
        let resp = self
            .send_request(
                "session/load",
                Some(build_session_params(
                    &self.session_params,
                    self.session_context.as_ref(),
                    cwd,
                    Some(session_id),
                    &self.mcp_servers,
                )),
            )
            .await?;
        // Accept any non-error response as success
        if resp.error.is_some() {
            return Err(anyhow!("session/load rejected"));
        }
        info!(session_id, "session loaded");
        self.acp_session_id = Some(session_id.to_string());
        if let Some(result) = resp.result.as_ref() {
            self.config_options = parse_config_options(result);
        }
        Ok(())
    }

    /// Kill the entire process group: SIGTERM → SIGKILL.
    /// Uses std::thread (not tokio::spawn) so SIGKILL fires even during
    /// runtime shutdown or panic unwinding.
    fn kill_process_group(&mut self) {
        let pgid = match self.child_pgid {
            Some(pid) if pid > 0 => pid,
            _ => return,
        };
        #[cfg(unix)]
        {
            // Stage 1: SIGTERM the process group
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
            // Stage 2: SIGKILL after brief grace (std::thread survives runtime shutdown)
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            });
        }
        #[cfg(not(unix))]
        {
            let _ = pgid; // suppress unused warning on Windows
        }
    }
}

impl Drop for AcpConnection {
    fn drop(&mut self) {
        self._reader_handle.abort();
        if let Some(handle) = self._stderr_handle.take() {
            handle.abort();
        }
        self.kill_process_group();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_agent_env, build_permission_response, build_session_params, mcp_servers_json,
        pick_best_option,
    };
    use crate::config::AgentMcpServerConfig;
    use serde_json::json;

    #[test]
    fn picks_allow_always_over_other_options() {
        let options = vec![
            json!({"kind": "allow_once", "optionId": "once"}),
            json!({"kind": "allow_always", "optionId": "always"}),
            json!({"kind": "reject_once", "optionId": "reject"}),
        ];

        assert_eq!(pick_best_option(&options), Some("always".to_string()));
    }

    #[test]
    fn falls_back_to_first_unknown_non_reject_kind() {
        let options = vec![
            json!({"kind": "reject_once", "optionId": "reject"}),
            json!({"kind": "workspace_write", "optionId": "workspace-write"}),
        ];

        assert_eq!(
            pick_best_option(&options),
            Some("workspace-write".to_string())
        );
    }

    #[test]
    fn selects_bypass_permissions_for_exit_plan_mode() {
        let options = vec![
            json!({"optionId": "bypassPermissions", "kind": "allow_always"}),
            json!({"optionId": "acceptEdits", "kind": "allow_always"}),
            json!({"optionId": "default", "kind": "allow_once"}),
            json!({"optionId": "plan", "kind": "reject_once"}),
        ];

        assert_eq!(
            pick_best_option(&options),
            Some("bypassPermissions".to_string())
        );
    }

    #[test]
    fn returns_none_when_only_reject_options_exist() {
        let options = vec![
            json!({"kind": "reject_once", "optionId": "reject-once"}),
            json!({"kind": "reject_always", "optionId": "reject-always"}),
        ];

        assert_eq!(pick_best_option(&options), None);
    }

    #[test]
    fn builds_cancelled_outcome_when_no_selectable_option_exists() {
        let response = build_permission_response(Some(&json!({
            "options": [
                {"kind": "reject_once", "optionId": "reject-once"}
            ]
        })));

        assert_eq!(response, json!({"outcome": {"outcome": "cancelled"}}));
    }

    #[test]
    fn builds_cancelled_when_options_array_is_empty() {
        let response = build_permission_response(Some(&json!({
            "options": []
        })));

        assert_eq!(response, json!({"outcome": {"outcome": "cancelled"}}));
    }

    #[test]
    fn falls_back_to_allow_always_when_options_are_missing() {
        let response = build_permission_response(Some(&json!({
            "toolCall": {"title": "legacy"}
        })));

        assert_eq!(
            response,
            json!({"outcome": {"outcome": "selected", "optionId": "allow_always"}})
        );
    }

    #[test]
    fn falls_back_to_allow_always_when_params_is_none() {
        let response = build_permission_response(None);

        assert_eq!(
            response,
            json!({"outcome": {"outcome": "selected", "optionId": "allow_always"}})
        );
    }

    #[test]
    fn explicit_env_takes_precedence_over_inherit_env() {
        let key = "OAB_TEST_PRECEDENCE";
        std::env::set_var(key, "from_process");
        let mut explicit = std::collections::HashMap::new();
        explicit.insert(key.to_string(), "from_config".to_string());
        let inherit = vec![key.to_string()];

        let (result, inherited) = build_agent_env(&explicit, &inherit);

        assert_eq!(result.get(key).unwrap(), "from_config");
        assert!(!inherited.contains(&key.to_string()));
        std::env::remove_var(key);
    }

    #[test]
    fn inherit_env_copies_from_process() {
        let key = "OAB_TEST_INHERIT";
        std::env::set_var(key, "process_value");
        let explicit = std::collections::HashMap::new();
        let inherit = vec![key.to_string()];

        let (result, inherited) = build_agent_env(&explicit, &inherit);

        assert_eq!(result.get(key).unwrap(), "process_value");
        assert!(inherited.contains(&key.to_string()));
        std::env::remove_var(key);
    }

    #[test]
    fn inherit_env_skips_missing_vars() {
        let explicit = std::collections::HashMap::new();
        let inherit = vec!["OAB_TEST_NONEXISTENT_VAR_12345".to_string()];

        let (result, inherited) = build_agent_env(&explicit, &inherit);

        assert!(!result.contains_key("OAB_TEST_NONEXISTENT_VAR_12345"));
        assert!(inherited.is_empty());
    }

    #[test]
    fn mcp_servers_json_empty_preserves_legacy_empty_array() {
        assert_eq!(mcp_servers_json(&[]), json!([]));
    }

    #[test]
    fn mcp_servers_json_uses_sequence_items() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer secret".to_string());
        let servers = vec![AgentMcpServerConfig {
            name: "openab_context".into(),
            transport: "http".into(),
            url: "http://openab:18080/mcp".into(),
            headers,
            allowed_tools: vec!["handoff_to_thread".into()],
        }];

        let value = mcp_servers_json(&servers);
        assert_eq!(
            value,
            json!([
                {
                    "name": "openab_context",
                    "type": "http",
                    "url": "http://openab:18080/mcp",
                    "headers": {"Authorization": "Bearer secret"},
                    "tool_filter": {"allow": ["handoff_to_thread"]},
                    "allowedTools": ["handoff_to_thread"]
                }
            ])
        );
    }

    #[test]
    fn build_session_params_merges_extra_fields() {
        let mut extra = serde_json::Map::new();
        extra.insert("permissionMode".into(), json!("acceptEdits"));
        extra.insert("maxTurns".into(), json!(3));

        let params = build_session_params(&extra, None, "/work", Some("sess-1"), &json!([]));

        assert_eq!(
            params,
            json!({
                "cwd": "/work",
                "sessionId": "sess-1",
                "mcpServers": [],
                "permissionMode": "acceptEdits",
                "maxTurns": 3
            })
        );
    }

    #[test]
    fn build_session_params_core_fields_take_precedence() {
        let mut extra = serde_json::Map::new();
        extra.insert("cwd".into(), json!("/wrong"));
        extra.insert("sessionId".into(), json!("wrong"));
        extra.insert("mcpServers".into(), json!({"wrong": true}));

        let params = build_session_params(&extra, None, "/work", Some("sess-1"), &json!([]));

        assert_eq!(params["cwd"], json!("/work"));
        assert_eq!(params["sessionId"], json!("sess-1"));
        assert_eq!(params["mcpServers"], json!([]));
    }

    #[test]
    fn build_session_params_includes_openab_session_context() {
        let extra = serde_json::Map::new();
        let context = json!({
            "platform": "discord",
            "channelId": "123",
            "threadId": null,
            "parentId": null,
            "channelKind": "normal"
        });

        let params = build_session_params(&extra, Some(&context), "/work", None, &json!([]));

        assert_eq!(params["openabSession"], context);
    }
}

#[cfg(test)]
mod reader_loop_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::io::{duplex, AsyncWriteExt};
    use tokio::sync::{mpsc, oneshot, Mutex};

    /// #732 stale-id path: when a response arrives for an id the broker has
    /// already abandoned, the reader must (a) not crash, (b) leave `pending`
    /// untouched, and (c) still forward the message to whoever is currently
    /// subscribed — the adapter recv loop is responsible for filtering by
    /// request_id so the stray response never leaks into the next prompt.
    #[tokio::test]
    async fn stale_id_response_is_forwarded_without_pending_entry() {
        let (mut agent_stdout_writer, agent_stdout_reader) = duplex(8 * 1024);
        let (agent_stdin_writer, _agent_stdin_reader) = duplex(8 * 1024);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(None));

        let (sub_tx, mut sub_rx) = mpsc::unbounded_channel();
        *notify_tx.lock().await = Some(sub_tx);

        let writer = Arc::new(Mutex::new(agent_stdin_writer));
        let handle = tokio::spawn(run_reader_loop(
            agent_stdout_reader,
            writer,
            pending.clone(),
            notify_tx.clone(),
        ));

        let stale = b"{\"jsonrpc\":\"2.0\",\"id\":42,\"result\":{\"stopReason\":\"ok\"}}\n";
        agent_stdout_writer.write_all(stale).await.unwrap();
        agent_stdout_writer.flush().await.unwrap();

        let forwarded = tokio::time::timeout(std::time::Duration::from_secs(2), sub_rx.recv())
            .await
            .expect("subscriber should receive stale message before timeout")
            .expect("subscriber channel should not be closed");
        assert_eq!(forwarded.id, Some(42));
        assert!(pending.lock().await.is_empty());

        drop(agent_stdout_writer);
        handle.await.unwrap();
    }

    /// Matched-id path: when a response's id is in `pending`, the loop must
    /// resolve the oneshot AND forward a copy to the subscriber so the
    /// adapter's recv loop sees the completion. Guards against regressions
    /// that would suppress the forward branch while keeping resolve.
    #[tokio::test]
    async fn matched_id_response_resolves_pending_and_forwards() {
        let (mut agent_stdout_writer, agent_stdout_reader) = duplex(8 * 1024);
        let (agent_stdin_writer, _agent_stdin_reader) = duplex(8 * 1024);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let notify_tx: Arc<Mutex<Option<mpsc::UnboundedSender<JsonRpcMessage>>>> =
            Arc::new(Mutex::new(None));

        let (resp_tx, resp_rx) = oneshot::channel();
        pending.lock().await.insert(7, resp_tx);

        let (sub_tx, mut sub_rx) = mpsc::unbounded_channel();
        *notify_tx.lock().await = Some(sub_tx);

        let writer = Arc::new(Mutex::new(agent_stdin_writer));
        let handle = tokio::spawn(run_reader_loop(
            agent_stdout_reader,
            writer,
            pending.clone(),
            notify_tx.clone(),
        ));

        let payload = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"stopReason\":\"end_turn\"}}\n";
        agent_stdout_writer.write_all(payload).await.unwrap();
        agent_stdout_writer.flush().await.unwrap();

        let resolved = tokio::time::timeout(std::time::Duration::from_secs(2), resp_rx)
            .await
            .expect("oneshot should resolve")
            .expect("oneshot should not be cancelled");
        assert_eq!(resolved.id, Some(7));

        let forwarded = tokio::time::timeout(std::time::Duration::from_secs(2), sub_rx.recv())
            .await
            .expect("subscriber should receive forwarded copy")
            .expect("subscriber channel should not be closed");
        assert_eq!(forwarded.id, Some(7));
        assert!(pending.lock().await.is_empty());

        drop(agent_stdout_writer);
        handle.await.unwrap();
    }
}
