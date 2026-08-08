use crate::config::UniverWorkspaceConfig;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::header::COOKIE;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

#[derive(Default)]
struct WorkspaceCache {
    user_ids: HashMap<String, String>,
    editor_memberships: HashSet<(String, String)>,
}

/// Resolves Discord users, caches their Workspace user IDs, and grants Team Space access.
///
/// The API key and bot cookie are retained only by this client and are never
/// exposed to the ACP subprocess or included in log fields. User login cookies
/// returned by Workspace are not retained.
pub struct DiscordBotLoginClient {
    base_url: String,
    api_key: String,
    bot_cookie: String,
    channel_spaces: RwLock<HashMap<String, String>>,
    channel_spaces_path: PathBuf,
    transport: Arc<dyn BotLoginTransport>,
    cache: Mutex<WorkspaceCache>,
}

#[async_trait]
trait BotLoginTransport: Send + Sync {
    async fn login(&self, endpoint: &str, api_key: &str, discord_user_id: &str) -> Result<String>;

    async fn grant_editor(&self, endpoint: &str, bot_cookie: &str) -> Result<()>;

    async fn create_team_space(
        &self,
        endpoint: &str,
        bot_cookie: &str,
        name: &str,
    ) -> Result<String>;
}

struct HttpBotLoginTransport {
    client: reqwest::Client,
}

#[async_trait]
impl BotLoginTransport for HttpBotLoginTransport {
    async fn login(&self, endpoint: &str, api_key: &str, discord_user_id: &str) -> Result<String> {
        let response = self
            .client
            .post(endpoint)
            .header("x-api-key", api_key)
            .json(&LoginRequest { discord_user_id })
            .send()
            .await
            .context("Discord bot login request failed")?;

        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("Discord bot login returned HTTP {status}"));
        }
        let session: AuthenticatedSession = response
            .json()
            .await
            .context("Discord bot login response was not a valid authenticated session")?;
        anyhow::ensure!(
            !session.user.id.trim().is_empty() && !session.user.id.contains(['/', '?', '#']),
            "Discord bot login response user.id must be a non-empty URL path segment"
        );
        Ok(session.user.id)
    }

    async fn grant_editor(&self, endpoint: &str, bot_cookie: &str) -> Result<()> {
        let response = self
            .client
            .put(endpoint)
            .header(COOKIE, bot_cookie)
            .json(&MembershipRequest { role: "editor" })
            .send()
            .await
            .context("Workspace Team Space editor grant request failed")?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!(
                "Workspace Team Space editor grant returned HTTP {status}"
            ));
        }
        let membership: MembershipResponse = response
            .json()
            .await
            .context("Workspace Team Space editor grant response was invalid")?;
        anyhow::ensure!(
            membership.role == "editor",
            "Workspace Team Space member role was not editor"
        );
        Ok(())
    }

    async fn create_team_space(
        &self,
        endpoint: &str,
        bot_cookie: &str,
        name: &str,
    ) -> Result<String> {
        let response = self
            .client
            .post(endpoint)
            .header(COOKIE, bot_cookie)
            .json(&CreateTeamSpaceRequest {
                name,
                public_read: true,
            })
            .send()
            .await
            .context("Workspace Team Space create request failed")?;
        let status = response.status();
        if status != reqwest::StatusCode::CREATED {
            return Err(anyhow!(
                "Workspace Team Space create returned HTTP {status}"
            ));
        }
        let space: CreatedTeamSpace = response
            .json()
            .await
            .context("Workspace Team Space create response was invalid")?;
        validate_space_id(&space.id)
            .context("Workspace Team Space create response id was invalid")?;
        Ok(space.id)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginRequest<'a> {
    discord_user_id: &'a str,
}

#[derive(Deserialize)]
struct AuthenticatedSession {
    user: AuthenticatedUser,
}

#[derive(Deserialize)]
struct AuthenticatedUser {
    id: String,
}

#[derive(Serialize)]
struct MembershipRequest<'a> {
    role: &'a str,
}

#[derive(Deserialize)]
struct MembershipResponse {
    role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateTeamSpaceRequest<'a> {
    name: &'a str,
    public_read: bool,
}

#[derive(Deserialize)]
struct CreatedTeamSpace {
    id: String,
}

impl DiscordBotLoginClient {
    pub fn new(config: UniverWorkspaceConfig) -> Result<Self> {
        let openab_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join(".openab");
        std::fs::create_dir_all(&openab_dir).with_context(|| {
            format!(
                "failed to create OpenAB state directory {}",
                openab_dir.display()
            )
        })?;
        Self::new_with_path(config, openab_dir.join("channel_space.json"))
    }

    fn new_with_path(config: UniverWorkspaceConfig, channel_spaces_path: PathBuf) -> Result<Self> {
        let host = config.host.trim().trim_end_matches('/');
        anyhow::ensure!(
            host.starts_with("http://") || host.starts_with("https://"),
            "univer_workspace.host must start with http:// or https://"
        );
        anyhow::ensure!(
            !config.api_key.trim().is_empty(),
            "univer_workspace.api_key must not be empty"
        );
        anyhow::ensure!(
            !config.bot_cookie.trim().is_empty(),
            "univer_workspace.bot_cookie must not be empty"
        );
        let channel_spaces = load_channel_spaces(&channel_spaces_path)?;
        info!(
            mapping_count = channel_spaces.len(),
            path = %channel_spaces_path.display(),
            "Univer Workspace integration configured"
        );
        debug!(
            mappings = ?channel_spaces,
            "loaded Discord channel to Workspace Space mappings"
        );
        Ok(Self {
            base_url: host.to_string(),
            api_key: config.api_key,
            bot_cookie: config.bot_cookie,
            channel_spaces: RwLock::new(channel_spaces),
            channel_spaces_path,
            transport: Arc::new(HttpBotLoginTransport {
                client: reqwest::Client::new(),
            }),
            cache: Mutex::new(WorkspaceCache::default()),
        })
    }

    /// Resolve a Discord root channel to a Team Space and ensure the sender is an editor.
    pub async fn provision_editor(
        &self,
        discord_user_id: &str,
        discord_channel_id: &str,
    ) -> Result<Option<String>> {
        let channel_spaces = self.channel_spaces.read().await;
        let Some(space_id) = channel_spaces.get(discord_channel_id).cloned() else {
            warn!(
                discord_channel_id,
                mapping_count = channel_spaces.len(),
                "no Workspace Space mapping for Discord root channel"
            );
            return Ok(None);
        };
        drop(channel_spaces);
        debug!(
            discord_channel_id,
            space_id, discord_user_id, "resolved Discord sender to Workspace Space"
        );

        // Keep the lock through the rare login/grant requests so concurrent first
        // messages cannot issue duplicate membership writes.
        let mut cache = self.cache.lock().await;
        let user_id = match cache.user_ids.get(discord_user_id) {
            Some(user_id) => {
                debug!(
                    discord_user_id,
                    workspace_user_id = user_id,
                    "Workspace user ID cache hit; skipping bot login"
                );
                user_id.clone()
            }
            _ => {
                info!(
                    discord_user_id,
                    "Workspace user ID cache miss; performing bot login"
                );
                let user_id = self
                    .transport
                    .login(
                        &format!("{}/api/auth/discord/bot-login", self.base_url),
                        &self.api_key,
                        discord_user_id,
                    )
                    .await?;
                cache
                    .user_ids
                    .insert(discord_user_id.to_string(), user_id.clone());
                debug!(
                    discord_user_id,
                    workspace_user_id = user_id,
                    "cached Workspace user ID from bot login"
                );
                user_id
            }
        };

        let membership_key = (space_id.clone(), user_id.clone());
        if !cache.editor_memberships.contains(&membership_key) {
            info!(
                discord_user_id,
                workspace_user_id = user_id,
                space_id,
                "granting Workspace Team Space editor access"
            );
            let endpoint = format!(
                "{}/api/team-spaces/{space_id}/members/{user_id}",
                self.base_url
            );
            self.transport
                .grant_editor(&endpoint, &self.bot_cookie)
                .await?;
            cache.editor_memberships.insert(membership_key);
        } else {
            debug!(
                discord_user_id,
                workspace_user_id = user_id,
                space_id,
                "Workspace editor membership cache hit; skipping grant request"
            );
        }
        Ok(Some(space_id))
    }

    /// Bind a Discord root channel to a Workspace Team Space and persist it atomically.
    pub async fn bind_channel_space(&self, discord_channel_id: &str, space_id: &str) -> Result<()> {
        validate_discord_channel_id(discord_channel_id)?;
        validate_space_id(space_id)?;

        let mut channel_spaces = self.channel_spaces.write().await;
        let mut updated = channel_spaces.clone();
        updated.insert(discord_channel_id.to_string(), space_id.to_string());
        persist_channel_spaces(&self.channel_spaces_path, &updated)?;
        *channel_spaces = updated;
        info!(
            discord_channel_id,
            space_id,
            path = %self.channel_spaces_path.display(),
            "bound Discord root channel to Workspace Space"
        );
        Ok(())
    }

    /// Create and bind a Team Space when the Discord root channel is not yet bound.
    /// Returns `None` when another binding already exists.
    pub async fn init_channel_space(
        &self,
        discord_channel_id: &str,
        channel_name: &str,
    ) -> Result<Option<String>> {
        validate_discord_channel_id(discord_channel_id)?;
        let channel_name = channel_name.trim();
        anyhow::ensure!(
            !channel_name.is_empty() && channel_name.chars().count() <= 100,
            "Discord channel name must contain 1 to 100 characters"
        );

        // Keep the write lock through create + persist so concurrent /init calls
        // cannot create duplicate Spaces for the same channel.
        let mut channel_spaces = self.channel_spaces.write().await;
        if channel_spaces.contains_key(discord_channel_id) {
            return Ok(None);
        }

        let space_id = self
            .transport
            .create_team_space(
                &format!("{}/api/team-spaces", self.base_url),
                &self.bot_cookie,
                channel_name,
            )
            .await?;
        let mut updated = channel_spaces.clone();
        updated.insert(discord_channel_id.to_string(), space_id.clone());
        persist_channel_spaces(&self.channel_spaces_path, &updated)?;
        *channel_spaces = updated;
        info!(
            discord_channel_id,
            space_id,
            channel_name,
            path = %self.channel_spaces_path.display(),
            "created and bound Workspace Space for Discord root channel"
        );
        Ok(Some(space_id))
    }
}

fn validate_discord_channel_id(discord_channel_id: &str) -> Result<()> {
    anyhow::ensure!(
        !discord_channel_id.starts_with('0')
            && !discord_channel_id.is_empty()
            && discord_channel_id.len() <= 20
            && discord_channel_id.bytes().all(|byte| byte.is_ascii_digit()),
        "Discord channel ID must be numeric"
    );
    Ok(())
}

fn validate_space_id(space_id: &str) -> Result<()> {
    anyhow::ensure!(
        !space_id.trim().is_empty() && !space_id.contains(['/', '?', '#']),
        "Workspace Space ID must be a non-empty URL path segment"
    );
    Ok(())
}

fn load_channel_spaces(path: &Path) -> Result<HashMap<String, String>> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let mappings: HashMap<String, String> = serde_json::from_slice(&data)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    for (discord_channel_id, space_id) in &mappings {
        validate_discord_channel_id(discord_channel_id)
            .with_context(|| format!("invalid Discord channel ID in {}", path.display()))?;
        validate_space_id(space_id)
            .with_context(|| format!("invalid Workspace Space ID in {}", path.display()))?;
    }
    Ok(mappings)
}

fn persist_channel_spaces(path: &Path, mappings: &HashMap<String, String>) -> Result<()> {
    let data =
        serde_json::to_vec_pretty(mappings).context("failed to serialize channel mappings")?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("channel mapping path has no parent directory"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    tmp.write_all(&data)
        .context("failed to write channel mappings")?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("failed to persist {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockTransport {
        login_requests: AtomicUsize,
        grant_requests: AtomicUsize,
        create_requests: AtomicUsize,
    }

    #[async_trait]
    impl BotLoginTransport for MockTransport {
        async fn login(
            &self,
            endpoint: &str,
            api_key: &str,
            discord_user_id: &str,
        ) -> Result<String> {
            assert_eq!(
                endpoint,
                "https://workspace.example.com/api/auth/discord/bot-login"
            );
            assert_eq!(api_key, "test-api-key");
            assert_eq!(discord_user_id, "123456789");
            self.login_requests.fetch_add(1, Ordering::SeqCst);
            Ok("workspace-user-1".into())
        }

        async fn grant_editor(&self, endpoint: &str, bot_cookie: &str) -> Result<()> {
            assert_eq!(
                endpoint,
                "https://workspace.example.com/api/team-spaces/space-1/members/workspace-user-1"
            );
            assert_eq!(bot_cookie, "workspace_session=bot-cookie");
            self.grant_requests.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn create_team_space(
            &self,
            endpoint: &str,
            bot_cookie: &str,
            name: &str,
        ) -> Result<String> {
            assert_eq!(endpoint, "https://workspace.example.com/api/team-spaces");
            assert_eq!(bot_cookie, "workspace_session=bot-cookie");
            assert_eq!(name, "general");
            self.create_requests.fetch_add(1, Ordering::SeqCst);
            Ok("created-space".into())
        }
    }

    #[test]
    fn parses_workspace_user_id_and_serializes_editor_role() {
        let session: AuthenticatedSession = serde_json::from_value(serde_json::json!({
            "authenticated": true,
            "user": {"id": "workspace-user-1"}
        }))
        .unwrap();
        assert_eq!(session.user.id, "workspace-user-1");
        assert_eq!(
            serde_json::to_value(MembershipRequest { role: "editor" }).unwrap(),
            serde_json::json!({"role": "editor"})
        );
        assert_eq!(
            serde_json::to_value(CreateTeamSpaceRequest {
                name: "general",
                public_read: true,
            })
            .unwrap(),
            serde_json::json!({"name": "general", "publicRead": true})
        );
    }

    #[tokio::test]
    async fn provisions_editor_and_caches_user_id_and_membership() {
        let transport = Arc::new(MockTransport {
            login_requests: AtomicUsize::new(0),
            grant_requests: AtomicUsize::new(0),
            create_requests: AtomicUsize::new(0),
        });
        let client = DiscordBotLoginClient {
            base_url: "https://workspace.example.com".into(),
            api_key: "test-api-key".into(),
            bot_cookie: "workspace_session=bot-cookie".into(),
            channel_spaces: RwLock::new(HashMap::from([("1234567890".into(), "space-1".into())])),
            channel_spaces_path: tempfile::tempdir()
                .unwrap()
                .path()
                .join("channel_space.json"),
            transport: transport.clone(),
            cache: Mutex::new(WorkspaceCache::default()),
        };
        assert_eq!(
            client
                .provision_editor("123456789", "1234567890")
                .await
                .unwrap()
                .as_deref(),
            Some("space-1")
        );
        client
            .provision_editor("123456789", "1234567890")
            .await
            .unwrap();
        assert_eq!(transport.login_requests.load(Ordering::SeqCst), 1);
        assert_eq!(transport.grant_requests.load(Ordering::SeqCst), 1);

        assert_eq!(
            client
                .provision_editor("123456789", "unmapped")
                .await
                .unwrap(),
            None
        );
        assert_eq!(transport.login_requests.load(Ordering::SeqCst), 1);
        assert_eq!(transport.grant_requests.load(Ordering::SeqCst), 1);

        assert_eq!(
            client
                .provision_editor("unmapped-user", "unmapped")
                .await
                .unwrap(),
            None
        );
        let cache = client.cache.lock().await;
        assert!(!cache.user_ids.contains_key("unmapped-user"));
        drop(cache);
        assert_eq!(transport.login_requests.load(Ordering::SeqCst), 1);
        assert_eq!(transport.grant_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn init_creates_and_persists_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel_space.json");
        let transport = Arc::new(MockTransport {
            login_requests: AtomicUsize::new(0),
            grant_requests: AtomicUsize::new(0),
            create_requests: AtomicUsize::new(0),
        });
        let client = Arc::new(DiscordBotLoginClient {
            base_url: "https://workspace.example.com".into(),
            api_key: "test-api-key".into(),
            bot_cookie: "workspace_session=bot-cookie".into(),
            channel_spaces: RwLock::new(HashMap::new()),
            channel_spaces_path: path.clone(),
            transport: transport.clone(),
            cache: Mutex::new(WorkspaceCache::default()),
        });

        let (first, second) = tokio::join!(
            client.init_channel_space("1234567890", "general"),
            client.init_channel_space("1234567890", "general")
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(outcomes.iter().filter(|value| value.is_some()).count(), 1);
        assert_eq!(transport.create_requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            serde_json::from_slice::<HashMap<String, String>>(&std::fs::read(path).unwrap())
                .unwrap(),
            HashMap::from([("1234567890".into(), "created-space".into())])
        );
    }

    #[tokio::test]
    async fn binds_channel_space_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel_space.json");
        let config = UniverWorkspaceConfig {
            host: "https://workspace.example.com".into(),
            api_key: "test-api-key".into(),
            bot_cookie: "workspace_session=bot-cookie".into(),
        };
        let client = DiscordBotLoginClient::new_with_path(config.clone(), path.clone()).unwrap();

        client
            .bind_channel_space("1234567890", "space-1")
            .await
            .unwrap();
        client
            .bind_channel_space("1234567890", "space-2")
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<HashMap<String, String>>(&std::fs::read(&path).unwrap())
                .unwrap(),
            HashMap::from([("1234567890".into(), "space-2".into())])
        );

        let reloaded = DiscordBotLoginClient::new_with_path(config, path).unwrap();
        assert_eq!(
            reloaded
                .channel_spaces
                .read()
                .await
                .get("1234567890")
                .map(String::as_str),
            Some("space-2")
        );
    }

    #[tokio::test]
    async fn rejects_invalid_binding_without_changing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel_space.json");
        let client = DiscordBotLoginClient::new_with_path(
            UniverWorkspaceConfig {
                host: "https://workspace.example.com".into(),
                api_key: "test-api-key".into(),
                bot_cookie: "workspace_session=bot-cookie".into(),
            },
            path.clone(),
        )
        .unwrap();

        assert!(client
            .bind_channel_space("1234567890", "bad/space")
            .await
            .is_err());
        assert!(!path.exists());
        assert!(client.channel_spaces.read().await.is_empty());
    }
}
