use crate::acp::connection::{AcpConnection, AcpWriter};
use crate::acp::protocol::ConfigOption;
use crate::config::AgentConfig;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;
use tracing::{info, warn};

/// Combined state protected by a single lock to prevent deadlocks.
/// Lock ordering: never await a per-connection mutex while holding `state`.
struct PoolState {
    /// Active connections: thread_key → AcpConnection handle.
    active: HashMap<String, Arc<Mutex<AcpConnection>>>,
    /// Lock-free cancel handles: thread_key → (writer, session_id).
    /// Stored separately so cancel can work without locking the connection.
    cancel_handles: HashMap<String, (AcpWriter, String)>,
    /// Suspended sessions: thread_key → ACP sessionId.
    /// Used at runtime to decide which thread can be resumed via `session/load`
    /// because it no longer has a live in-memory connection.
    suspended: HashMap<String, String>,
    /// Persisted resumable sessions: thread_key → ACP sessionId.
    /// Includes both suspended sessions and active sessions so a process restart
    /// can recover any live thread via `session/load`.
    persisted: HashMap<String, String>,
    /// Serializes create/resume work per thread so rapid same-thread requests
    /// cannot race each other into duplicate `session/load` attempts.
    creating: HashMap<String, Arc<Mutex<()>>>,
    /// Per-session working directory overrides (from control directives).
    /// thread_key → canonical workspace path.
    session_workdirs: HashMap<String, String>,
}

pub struct SessionPool {
    state: RwLock<PoolState>,
    config: AgentConfig,
    max_sessions: usize,
    mapping_path: PathBuf,
    meta_path: PathBuf,
}

type EvictionCandidate = (String, Arc<Mutex<AcpConnection>>, Instant, Option<String>);

fn remove_if_same_handle<T>(
    map: &mut HashMap<String, Arc<Mutex<T>>>,
    key: &str,
    expected: &Arc<Mutex<T>>,
) -> Option<Arc<Mutex<T>>> {
    let should_remove = map
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, expected));
    if should_remove {
        map.remove(key)
    } else {
        None
    }
}

fn get_or_insert_gate(map: &mut HashMap<String, Arc<Mutex<()>>>, key: &str) -> Arc<Mutex<()>> {
    map.entry(key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

impl SessionPool {
    pub fn new(config: AgentConfig, max_sessions: usize) -> Self {
        let openab_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join(".openab");
        let _ = std::fs::create_dir_all(&openab_dir);
        let mapping_path = openab_dir.join("thread_map.json");
        let meta_path = openab_dir.join("session_meta.json");
        let suspended = Self::load_mapping(&mapping_path);
        let session_workdirs = Self::load_mapping(&meta_path);
        Self {
            state: RwLock::new(PoolState {
                active: HashMap::new(),
                cancel_handles: HashMap::new(),
                persisted: suspended.clone(),
                suspended,
                creating: HashMap::new(),
                session_workdirs,
            }),
            config,
            max_sessions,
            mapping_path,
            meta_path,
        }
    }

    fn load_mapping(path: &Path) -> HashMap<String, String> {
        match std::fs::read_to_string(path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
                warn!(path = %path.display(), error = %e, "corrupt mapping file, starting fresh");
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        }
    }

    fn save_mapping(&self, persisted: &HashMap<String, String>) {
        let data = match serde_json::to_string_pretty(persisted) {
            Ok(d) => d,
            Err(e) => {
                warn!(error = %e, "failed to serialize thread mapping");
                return;
            }
        };
        let tmp = self.mapping_path.with_extension("json.tmp");
        if let Err(e) =
            std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &self.mapping_path))
        {
            warn!(path = %self.mapping_path.display(), error = %e, "failed to persist thread mapping");
        }
    }

    fn resolve_working_dir(&self, thread_id: &str) -> Result<String> {
        let base = PathBuf::from(&self.config.working_dir);

        if !self.config.per_session_working_dir {
            if self.config.transport == crate::config::AgentTransport::Stdio {
                anyhow::ensure!(
                    base.is_dir(),
                    "agent working_dir does not exist or is not a directory: {}",
                    base.display()
                );
            }
            return Ok(self.config.working_dir.clone());
        }

        let resolved = base.join(sanitize_session_dir_component(thread_id));
        if self.config.transport == crate::config::AgentTransport::Stdio {
            anyhow::ensure!(
                base.is_dir(),
                "agent working_dir does not exist or is not a directory: {}",
                base.display()
            );
            std::fs::create_dir_all(&resolved).map_err(|e| {
                anyhow!(
                    "failed to create per-session working_dir {}: {}",
                    resolved.display(),
                    e
                )
            })?;
        }
        Ok(resolved.to_string_lossy().into_owned())
    }

    fn save_meta(&self, workdirs: &HashMap<String, String>) {
        let data = match serde_json::to_string_pretty(workdirs) {
            Ok(d) => d,
            Err(e) => {
                warn!(error = %e, "failed to serialize session metadata");
                return;
            }
        };
        let tmp = self.meta_path.with_extension("json.tmp");
        if let Err(e) =
            std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &self.meta_path))
        {
            warn!(path = %self.meta_path.display(), error = %e, "failed to persist session metadata");
        }
    }

    /// Check if session state exists for this thread (active, suspended, or persisted).
    #[allow(dead_code)]
    pub async fn has_active_session(&self, thread_id: &str) -> bool {
        let state = self.state.read().await;
        // Any of these means the thread already has session state.
        if state.suspended.contains_key(thread_id) || state.persisted.contains_key(thread_id) {
            return true;
        }
        if let Some(conn) = state.active.get(thread_id) {
            match conn.try_lock() {
                Ok(c) => return c.alive(),
                Err(_) => return true, // lock held = connection busy streaming = alive
            }
        }
        false
    }

    pub async fn get_or_create(
        &self,
        thread_id: &str,
        working_dir_override: Option<&str>,
    ) -> Result<bool> {
        let create_gate = {
            let mut state = self.state.write().await;
            get_or_insert_gate(&mut state.creating, thread_id)
        };
        let _create_guard = create_gate.lock().await;

        let (existing, saved_session_id) = {
            let state = self.state.read().await;
            (
                state.active.get(thread_id).cloned(),
                state.suspended.get(thread_id).cloned(),
            )
        };

        let had_existing = existing.is_some();
        let mut saved_session_id = saved_session_id;
        if let Some(conn) = existing.clone() {
            let conn = conn.lock().await;
            if conn.alive() {
                return Ok(false);
            }
            if saved_session_id.is_none() {
                saved_session_id = conn.acp_session_id.clone();
            }
        }

        // Snapshot active handles so we can inspect them outside the state lock.
        let snapshot: Vec<(String, Arc<Mutex<AcpConnection>>)> = {
            let state = self.state.read().await;
            state
                .active
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        let mut eviction_candidate: Option<EvictionCandidate> = None;
        let mut skipped_locked_candidates = 0usize;
        for (key, conn) in snapshot {
            if key == thread_id {
                continue;
            }
            let conn_handle = Arc::clone(&conn);
            let Ok(conn) = conn.try_lock() else {
                skipped_locked_candidates += 1;
                continue;
            };
            let candidate = (
                key,
                conn_handle,
                conn.last_active,
                conn.acp_session_id.clone(),
            );
            match &eviction_candidate {
                Some((_, _, oldest_last_active, _)) if candidate.2 >= *oldest_last_active => {}
                _ => eviction_candidate = Some(candidate),
            }
        }

        // Resolve effective working directory: stored per-session > explicit override > global config.
        // Stored value has highest priority to enforce immutability (ADR §4.5).
        let stored_workdir = {
            let state = self.state.read().await;
            state.session_workdirs.get(thread_id).cloned()
        };

        let effective_workdir = if let Some(stored) = stored_workdir {
            stored
        } else if let Some(wd) = working_dir_override {
            wd.to_string()
        } else {
            self.resolve_working_dir(thread_id)?
        };

        // Build the replacement connection outside the state lock so one stuck
        // initialization does not block all unrelated sessions.
        let mut new_conn =
            AcpConnection::spawn(&self.config, &effective_workdir, thread_id).await?;

        new_conn.initialize().await?;

        let mut resumed = false;
        if let Some(ref sid) = saved_session_id {
            if new_conn.supports_load_session {
                match new_conn.session_load(sid, &effective_workdir).await {
                    Ok(()) => {
                        info!(thread_id, session_id = %sid, "session resumed via session/load");
                        resumed = true;
                    }
                    Err(e) => {
                        warn!(thread_id, session_id = %sid, error = %e, "session/load failed, creating new session");
                    }
                }
            }
        }

        if !resumed {
            new_conn.session_new(&effective_workdir).await?;
            // Surface the reset banner both for restored sessions and for stale
            // live entries that died before we could recover a resumable
            // session id. In both cases the caller is continuing after an
            // unexpected session loss.
            if had_existing || saved_session_id.is_some() {
                new_conn.session_reset = true;
            }
        }

        let cancel_handle = new_conn.cancel_handle();
        let cancel_session_id = new_conn.acp_session_id.clone().unwrap_or_default();
        let new_conn = Arc::new(Mutex::new(new_conn));

        let stale_conn = {
            let mut state = self.state.write().await;

            // Another task may have created a healthy connection while we were
            // initializing this one.
            if let Some(existing) = state.active.get(thread_id).cloned() {
                let Ok(existing) = existing.try_lock() else {
                    return Ok(false);
                };
                if existing.alive() {
                    return Ok(false);
                }
                warn!(thread_id, "stale connection, rebuilding");
                drop(existing);
                state.cancel_handles.remove(thread_id);
                state.active.remove(thread_id)
            } else {
                None
            }
        };
        if let Some(stale_conn) = stale_conn {
            let mut stale_conn = stale_conn.lock().await;
            stale_conn.close_transport().await;
        }

        let mut state = self.state.write().await;

        let mut retired = Vec::new();
        if state.active.len() >= self.max_sessions {
            if let Some((key, expected_conn, _, sid)) = eviction_candidate {
                if remove_if_same_handle(&mut state.active, &key, &expected_conn).is_some() {
                    state.cancel_handles.remove(&key);
                    retired.push(expected_conn);
                    info!(evicted = %key, "pool full, suspending oldest idle session");
                    if let Some(sid) = sid {
                        state.persisted.insert(key.clone(), sid.clone());
                        state.suspended.insert(key, sid);
                    } else {
                        state.persisted.remove(&key);
                    }
                } else {
                    warn!(evicted = %key, "pool full but eviction candidate changed before removal");
                }
            } else if skipped_locked_candidates > 0 {
                warn!(
                    max_sessions = self.max_sessions,
                    skipped_locked_candidates,
                    "pool full but all other sessions were busy during eviction scan"
                );
            }
        }

        if state.active.len() >= self.max_sessions {
            return Err(anyhow!("pool exhausted ({} sessions)", self.max_sessions));
        }

        if cancel_session_id.is_empty() {
            state.persisted.remove(thread_id);
        } else {
            state
                .persisted
                .insert(thread_id.to_string(), cancel_session_id.clone());
        }
        state.suspended.remove(thread_id);
        state.active.insert(thread_id.to_string(), new_conn);
        if !cancel_session_id.is_empty() {
            state
                .cancel_handles
                .insert(thread_id.to_string(), (cancel_handle, cancel_session_id));
        }
        self.save_mapping(&state.persisted);

        // Persist workspace override only after session spawn succeeded (口渡 F2).
        if working_dir_override.is_some() {
            state
                .session_workdirs
                .entry(thread_id.to_string())
                .or_insert_with(|| effective_workdir.clone());
            self.save_meta(&state.session_workdirs);
        }

        // Return true only for genuinely new sessions — not resumed or reconnected ones.
        // A session with prior state (saved_session_id or had_existing) is a resume,
        // even if we had to spawn a new ACP process. ADR §2.2: directives are first-message-only.
        let is_fresh = !had_existing && saved_session_id.is_none();
        drop(state);
        for conn in retired {
            let mut conn = conn.lock().await;
            conn.close_transport().await;
        }
        Ok(is_fresh)
    }

    /// Get mutable access to a connection. Caller must have called get_or_create first.
    ///
    /// Only the per-connection `Mutex` is held during `f`; the pool-level
    /// `RwLock` is acquired briefly (read-only) to look up the `Arc` and then
    /// released, so other connections can be used concurrently.
    pub async fn with_connection<F, R>(&self, thread_id: &str, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(
            &'a mut AcpConnection,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<R>> + Send + 'a>,
        >,
    {
        let conn = {
            let state = self.state.read().await;
            state
                .active
                .get(thread_id)
                .cloned()
                .ok_or_else(|| anyhow!("no connection for thread {thread_id}"))?
        };

        let mut conn = conn.lock().await;
        f(&mut conn).await
    }

    /// Get cached configOptions for a session (e.g. available models).
    pub async fn get_config_options(&self, thread_id: &str) -> Vec<ConfigOption> {
        let state = self.state.read().await;
        let conn = match state.active.get(thread_id) {
            Some(c) => c.clone(),
            None => return Vec::new(),
        };
        drop(state);
        let conn = conn.lock().await;
        conn.config_options.clone()
    }

    /// Set a config option (e.g. model) via ACP and return updated options.
    pub async fn set_config_option(
        &self,
        thread_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<ConfigOption>> {
        let conn = {
            let state = self.state.read().await;
            state
                .active
                .get(thread_id)
                .cloned()
                .ok_or_else(|| anyhow!("no connection for thread {thread_id}"))?
        };
        let mut conn = conn.lock().await;
        conn.set_config_option(config_id, value).await
    }

    /// Cancel the current in-flight operation for a session.
    /// Uses pre-stored cancel handles to avoid locking the connection (which is held during streaming).
    pub async fn cancel_session(&self, thread_id: &str) -> Result<()> {
        let (writer, session_id) = {
            let state = self.state.read().await;
            state
                .cancel_handles
                .get(thread_id)
                .cloned()
                .ok_or_else(|| anyhow!("no session for thread {thread_id}"))?
        };
        let data = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {"sessionId": session_id}
        }))?;
        tracing::info!(session_id, "sending session/cancel");
        writer.send_line(&data).await?;
        Ok(())
    }

    /// Reset a session: cancel any in-flight operation, remove the active connection,
    /// and clear all suspended state. The ACP process will be killed once the last
    /// Arc reference is dropped (after streaming finishes). The next message will
    /// trigger a fresh `get_or_create` with a new ACP session.
    pub async fn reset_session(&self, thread_id: &str) -> Result<()> {
        // Send session/cancel via the lock-free stdin handle first.
        // This stops in-flight streaming even while with_connection() holds the
        // connection mutex, so the old process finishes promptly.
        if let Some((writer, session_id)) = {
            let state = self.state.read().await;
            state.cancel_handles.get(thread_id).cloned()
        } {
            let data = serde_json::to_string(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "session/cancel",
                "params": {"sessionId": session_id}
            }))?;
            tracing::info!(session_id, "reset: sending session/cancel");
            let _ = writer.send_line(&data).await;
        }

        let mut state = self.state.write().await;
        let removed = state.active.remove(thread_id);
        let had_active = removed.is_some();
        state.cancel_handles.remove(thread_id);
        state.suspended.remove(thread_id);
        state.persisted.remove(thread_id);
        state.creating.remove(thread_id);
        state.session_workdirs.remove(thread_id);
        self.save_mapping(&state.persisted);
        self.save_meta(&state.session_workdirs);
        drop(state);
        if let Some(conn) = removed {
            let mut conn = conn.lock().await;
            conn.close_transport().await;
        }
        if had_active {
            info!(thread_id, "session reset");
            Ok(())
        } else {
            Err(anyhow!("no session for thread {thread_id}"))
        }
    }

    pub async fn cleanup_idle(&self, ttl: std::time::Duration) {
        let cutoff = Instant::now() - ttl;

        let snapshot: Vec<(String, Arc<Mutex<AcpConnection>>)> = {
            let state = self.state.read().await;
            state
                .active
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        let mut stale = Vec::new();
        for (key, conn) in snapshot {
            // Skip active sessions for this cleanup round instead of waiting on
            // their per-connection mutex. A busy session is not idle.
            let conn_handle = Arc::clone(&conn);
            let Ok(conn) = conn.try_lock() else {
                continue;
            };
            if conn.last_active < cutoff || !conn.alive() {
                stale.push((key, conn_handle, conn.acp_session_id.clone()));
            }
        }

        if stale.is_empty() {
            return;
        }

        let mut state = self.state.write().await;
        let mut retired = Vec::new();
        for (key, expected_conn, sid) in stale {
            if remove_if_same_handle(&mut state.active, &key, &expected_conn).is_some() {
                info!(thread_id = %key, "cleaning up idle session");
                state.cancel_handles.remove(&key);
                retired.push(expected_conn);
                if let Some(sid) = sid {
                    state.persisted.insert(key.clone(), sid.clone());
                    state.suspended.insert(key, sid);
                } else {
                    state.persisted.remove(&key);
                    state.session_workdirs.remove(&key);
                }
            }
        }
        self.save_mapping(&state.persisted);
        self.save_meta(&state.session_workdirs);
        drop(state);
        for conn in retired {
            let mut conn = conn.lock().await;
            conn.close_transport().await;
        }
    }

    pub async fn shutdown(&self) {
        // Snapshot active handles, then drop state lock before awaiting
        // per-connection mutexes (lock ordering: never hold state while
        // awaiting a connection lock).
        let snapshot: Vec<(String, Arc<Mutex<AcpConnection>>)> = {
            let state = self.state.read().await;
            state
                .active
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        let mut session_ids: Vec<(String, String)> = Vec::new();
        for (key, conn) in &snapshot {
            let conn = conn.lock().await;
            if let Some(sid) = conn.acp_session_id.clone() {
                session_ids.push((key.clone(), sid));
            }
        }

        let mut state = self.state.write().await;
        for (key, sid) in session_ids {
            state.persisted.insert(key.clone(), sid.clone());
            state.suspended.insert(key, sid);
        }
        self.save_mapping(&state.persisted);
        let count = state.active.len();
        state.active.clear();
        state.cancel_handles.clear();
        drop(state);
        for (_, conn) in snapshot {
            let mut conn = conn.lock().await;
            conn.close_transport().await;
        }
        info!(count, "pool shutdown complete");
    }
}

fn sanitize_session_dir_component(thread_id: &str) -> String {
    let mut sanitized = String::with_capacity(thread_id.len());
    for ch in thread_id.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }

    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        get_or_insert_gate, remove_if_same_handle, sanitize_session_dir_component, SessionPool,
    };
    use crate::config::{AgentConfig, AgentTransport};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn remove_if_same_handle_removes_matching_entry() {
        let expected = Arc::new(Mutex::new(1_u8));
        let mut map = HashMap::from([("thread".to_string(), Arc::clone(&expected))]);

        let removed = remove_if_same_handle(&mut map, "thread", &expected);

        assert!(removed.is_some());
        assert!(map.is_empty());
    }

    #[test]
    fn remove_if_same_handle_keeps_replaced_entry() {
        let stale = Arc::new(Mutex::new(1_u8));
        let fresh = Arc::new(Mutex::new(2_u8));
        let mut map = HashMap::from([("thread".to_string(), Arc::clone(&fresh))]);

        let removed = remove_if_same_handle(&mut map, "thread", &stale);

        assert!(removed.is_none());
        let current = map.get("thread").expect("entry should remain");
        assert!(Arc::ptr_eq(current, &fresh));
    }

    #[test]
    fn get_or_insert_gate_reuses_gate_for_same_thread() {
        let mut map = HashMap::new();

        let first = get_or_insert_gate(&mut map, "thread");
        let second = get_or_insert_gate(&mut map, "thread");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn persisted_mapping_can_include_active_and_suspended_sessions() {
        let persisted = HashMap::from([
            ("active-thread".to_string(), "session-active".to_string()),
            (
                "suspended-thread".to_string(),
                "session-suspended".to_string(),
            ),
        ]);

        let serialized =
            serde_json::to_string_pretty(&persisted).expect("serialize persisted mapping");
        let roundtrip: HashMap<String, String> =
            serde_json::from_str(&serialized).expect("deserialize persisted mapping");

        assert_eq!(
            roundtrip.get("active-thread"),
            Some(&"session-active".to_string())
        );
        assert_eq!(
            roundtrip.get("suspended-thread"),
            Some(&"session-suspended".to_string())
        );
    }

    #[test]
    fn sanitize_session_dir_component_replaces_separators() {
        assert_eq!(
            sanitize_session_dir_component("discord:1234567890"),
            "discord_1234567890"
        );
        assert_eq!(
            sanitize_session_dir_component("slack:C0123/U0456"),
            "slack_C0123_U0456"
        );
    }

    #[test]
    fn resolve_working_dir_uses_base_dir_by_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pool = SessionPool::new(
            AgentConfig {
                transport: AgentTransport::Stdio,
                command: "echo".into(),
                args: vec![],
                url: None,
                headers: HashMap::new(),
                working_dir: tmp.path().to_string_lossy().into_owned(),
                per_session_working_dir: false,
                env: HashMap::new(),
                inherit_env: vec![],
                mcp_servers: vec![],
                command_explicit: true,
            },
            1,
        );

        let resolved = pool
            .resolve_working_dir("discord:123")
            .expect("resolve default working dir");
        assert_eq!(PathBuf::from(resolved), tmp.path());
    }

    #[test]
    fn resolve_working_dir_creates_per_session_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pool = SessionPool::new(
            AgentConfig {
                transport: AgentTransport::Stdio,
                command: "echo".into(),
                args: vec![],
                url: None,
                headers: HashMap::new(),
                working_dir: tmp.path().to_string_lossy().into_owned(),
                per_session_working_dir: true,
                env: HashMap::new(),
                inherit_env: vec![],
                mcp_servers: vec![],
                command_explicit: true,
            },
            1,
        );

        let resolved = pool
            .resolve_working_dir("discord:123")
            .expect("resolve per-session working dir");
        let expected = tmp.path().join("discord_123");
        assert_eq!(PathBuf::from(&resolved), expected);
        assert!(expected.is_dir());
    }

    #[test]
    fn resolve_working_dir_stdio_requires_existing_base_dir() {
        let missing = "/tmp/openab-nonexistent-working-dir-stdio";
        let pool = SessionPool::new(
            AgentConfig {
                transport: AgentTransport::Stdio,
                command: "echo".into(),
                args: vec![],
                url: None,
                headers: HashMap::new(),
                working_dir: missing.into(),
                per_session_working_dir: false,
                env: HashMap::new(),
                inherit_env: vec![],
                mcp_servers: vec![],
                command_explicit: true,
            },
            1,
        );

        let err = pool
            .resolve_working_dir("discord:123")
            .expect_err("stdio should validate local working_dir");
        assert!(err.to_string().contains("working_dir does not exist"));
    }

    #[test]
    fn resolve_working_dir_websocket_skips_local_validation() {
        let missing = "/tmp/openab-nonexistent-working-dir-websocket";
        let pool = SessionPool::new(
            AgentConfig {
                transport: AgentTransport::WebSocket,
                command: String::new(),
                args: vec![],
                url: Some("ws://127.0.0.1:3000".into()),
                headers: HashMap::new(),
                working_dir: missing.into(),
                per_session_working_dir: false,
                env: HashMap::new(),
                inherit_env: vec![],
                mcp_servers: vec![],
                command_explicit: false,
            },
            1,
        );

        let resolved = pool
            .resolve_working_dir("discord:123")
            .expect("websocket should not validate local working_dir");
        assert_eq!(resolved, missing);
    }

    #[test]
    fn resolve_working_dir_websocket_does_not_create_per_session_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing_base = tmp.path().join("remote-only-base");
        let expected = missing_base.join("discord_123");
        let pool = SessionPool::new(
            AgentConfig {
                transport: AgentTransport::WebSocket,
                command: String::new(),
                args: vec![],
                url: Some("ws://127.0.0.1:3000".into()),
                headers: HashMap::new(),
                working_dir: missing_base.to_string_lossy().into_owned(),
                per_session_working_dir: true,
                env: HashMap::new(),
                inherit_env: vec![],
                mcp_servers: vec![],
                command_explicit: false,
            },
            1,
        );

        let resolved = pool
            .resolve_working_dir("discord:123")
            .expect("websocket should only derive the per-session cwd string");
        assert_eq!(PathBuf::from(&resolved), expected);
        assert!(!expected.exists());
        assert!(!missing_base.exists());
    }
}
