use crate::acp::ContentBlock;
use crate::adapter::{ChannelRef, ChatAdapter, MessageRef};
use crate::dispatch::{estimate_tokens, BufferedMessage, Dispatcher};
use crate::format;
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Clone)]
pub struct HandoffBroker {
    ttl: Duration,
    entries: Arc<Mutex<HashMap<String, HandoffEntry>>>,
}

pub struct HandoffRegistration {
    pub adapter: Arc<dyn ChatAdapter>,
    pub dispatcher: Arc<Dispatcher>,
    pub parent_channel: ChannelRef,
    pub trigger_msg: MessageRef,
    pub sender_json: String,
    pub sender_name: String,
    pub sender_id: String,
    pub original_prompt: String,
    pub extra_blocks: Vec<ContentBlock>,
    pub other_bot_present: bool,
}

struct HandoffEntry {
    adapter: Arc<dyn ChatAdapter>,
    dispatcher: Arc<Dispatcher>,
    parent_channel: ChannelRef,
    trigger_msg: MessageRef,
    sender_json: String,
    sender_name: String,
    sender_id: String,
    original_prompt: String,
    extra_blocks: Vec<ContentBlock>,
    other_bot_present: bool,
    expires_at: Instant,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HandoffStarted {
    pub status: String,
    pub platform: String,
    pub channel_id: String,
    pub thread_id: Option<String>,
    pub display: String,
}

impl HandoffBroker {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register(&self, registration: HandoffRegistration) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        let entry = HandoffEntry {
            adapter: registration.adapter,
            dispatcher: registration.dispatcher,
            parent_channel: registration.parent_channel,
            trigger_msg: registration.trigger_msg,
            sender_json: registration.sender_json,
            sender_name: registration.sender_name,
            sender_id: registration.sender_id,
            original_prompt: registration.original_prompt,
            extra_blocks: registration.extra_blocks,
            other_bot_present: registration.other_bot_present,
            expires_at: Instant::now() + self.ttl,
        };
        self.entries.lock().await.insert(token.clone(), entry);
        token
    }

    async fn consume(&self, token: &str) -> Result<HandoffEntry> {
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.remove(token) else {
            return Err(anyhow!("handoff token is invalid or already used"));
        };
        if Instant::now() > entry.expires_at {
            return Err(anyhow!("handoff token has expired"));
        }
        Ok(entry)
    }

    pub async fn start_thread_task(
        &self,
        token: &str,
        title: &str,
        prompt: &str,
    ) -> Result<HandoffStarted> {
        let title = title.trim();
        let prompt = prompt.trim();
        if title.is_empty() {
            return Err(anyhow!("handoff title must not be empty"));
        }
        if prompt.is_empty() {
            return Err(anyhow!("handoff prompt must not be empty"));
        }

        let entry = self.consume(token).await?;
        let thread_title = format::shorten_thread_name(title);
        let thread_channel = entry
            .adapter
            .create_thread(&entry.parent_channel, &entry.trigger_msg, &thread_title)
            .await
            .map_err(|e| anyhow!("failed to create handoff thread: {e}"))?;

        let child_prompt = build_child_prompt(prompt, &entry);
        let thread_id = thread_channel
            .thread_id
            .as_deref()
            .unwrap_or(&thread_channel.channel_id)
            .to_string();
        let thread_key =
            entry
                .dispatcher
                .key(entry.adapter.platform(), &thread_id, &entry.sender_id);
        let estimated_tokens = estimate_tokens(&child_prompt, &entry.extra_blocks);
        let msg = BufferedMessage {
            sender_json: entry.sender_json,
            sender_name: entry.sender_name,
            prompt: child_prompt,
            extra_blocks: entry.extra_blocks,
            trigger_msg: entry.trigger_msg,
            arrived_at: std::time::Instant::now(),
            estimated_tokens,
            other_bot_present: entry.other_bot_present,
            recipient: None,
            initial_reply_to: None,
            session_key_override: None,
        };

        entry
            .dispatcher
            .submit(
                thread_key,
                thread_channel.clone(),
                entry.adapter.clone(),
                msg,
            )
            .await
            .map_err(|e| anyhow!("failed to enqueue handoff task: {e}"))?;

        Ok(HandoffStarted {
            status: "started".into(),
            platform: thread_channel.platform.clone(),
            channel_id: thread_channel.channel_id.clone(),
            thread_id: thread_channel.thread_id.clone(),
            display: format_thread_display(&thread_channel),
        })
    }
}

fn build_child_prompt(prompt: &str, entry: &HandoffEntry) -> String {
    format!(
        "<openab_handoff_context>\nplatform: {}\nparent_channel_id: {}\ntrigger_message_id: {}\noriginal_sender: {}\noriginal_message:\n{}\n</openab_handoff_context>\n\n{}",
        entry.parent_channel.platform,
        entry.parent_channel.channel_id,
        entry.trigger_msg.message_id,
        entry.sender_name,
        entry.original_prompt,
        prompt,
    )
}

fn format_thread_display(channel: &ChannelRef) -> String {
    match channel.thread_id.as_deref() {
        Some(thread_id) => format!("{} thread {}", channel.platform, thread_id),
        None => format!("{} channel {}", channel.platform, channel.channel_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::TypingHandle;
    use crate::dispatch::DispatchTarget;
    use crate::reactions::StatusReactionController;
    use async_trait::async_trait;

    #[derive(Clone, Default)]
    struct MockAdapter {
        platform: &'static str,
        create_calls: Arc<Mutex<Vec<(ChannelRef, MessageRef, String)>>>,
    }

    #[async_trait]
    impl ChatAdapter for MockAdapter {
        fn platform(&self) -> &'static str {
            self.platform
        }

        fn message_limit(&self) -> usize {
            2000
        }

        async fn send_message(&self, channel: &ChannelRef, _content: &str) -> Result<MessageRef> {
            Ok(MessageRef {
                channel: channel.clone(),
                message_id: "sent".into(),
            })
        }

        async fn create_thread(
            &self,
            channel: &ChannelRef,
            trigger_msg: &MessageRef,
            title: &str,
        ) -> Result<ChannelRef> {
            self.create_calls.lock().await.push((
                channel.clone(),
                trigger_msg.clone(),
                title.to_string(),
            ));
            if self.platform == "slack" {
                Ok(ChannelRef {
                    platform: "slack".into(),
                    channel_id: "C1".into(),
                    thread_id: Some("111.222".into()),
                    parent_id: None,
                    origin_event_id: None,
                })
            } else {
                Ok(ChannelRef {
                    platform: "discord".into(),
                    channel_id: "thread".into(),
                    thread_id: None,
                    parent_id: Some("parent".into()),
                    origin_event_id: None,
                })
            }
        }

        async fn add_reaction(&self, _msg: &MessageRef, _emoji: &str) -> Result<()> {
            Ok(())
        }

        async fn remove_reaction(&self, _msg: &MessageRef, _emoji: &str) -> Result<()> {
            Ok(())
        }

        fn start_typing(&self, _channel: &ChannelRef) -> Option<Box<dyn TypingHandle>> {
            None
        }

        fn use_streaming(&self, _other_bot_present: bool) -> bool {
            false
        }
    }

    #[derive(Clone)]
    struct CapturedDispatch {
        session_key: String,
        content_blocks: Vec<ContentBlock>,
        thread_channel: ChannelRef,
    }

    #[derive(Clone, Default)]
    struct TestTarget {
        reactions: crate::config::ReactionsConfig,
        captured: Arc<Mutex<Vec<CapturedDispatch>>>,
    }

    #[async_trait]
    impl DispatchTarget for TestTarget {
        fn reactions_config(&self) -> &crate::config::ReactionsConfig {
            &self.reactions
        }

        fn workspace_aliases(&self) -> std::collections::HashMap<String, String> {
            std::collections::HashMap::new()
        }

        fn bot_home(&self) -> std::path::PathBuf {
            std::env::temp_dir()
        }

        async fn ensure_session(
            &self,
            _session_key: &str,
            _working_dir: Option<&str>,
        ) -> Result<bool> {
            Ok(true)
        }

        async fn reset_session(&self, _session_key: &str) {}

        async fn cancel_session(&self, _session_key: &str) -> Result<()> {
            Ok(())
        }

        async fn stream_prompt_blocks(
            &self,
            _adapter: &Arc<dyn ChatAdapter>,
            session_key: &str,
            content_blocks: Vec<ContentBlock>,
            thread_channel: &ChannelRef,
            _reactions: Arc<StatusReactionController>,
            _other_bot_present: bool,
            _recipient: Option<(String, String)>,
            _initial_reply_to: Option<String>,
        ) -> Result<()> {
            self.captured.lock().await.push(CapturedDispatch {
                session_key: session_key.to_string(),
                content_blocks,
                thread_channel: thread_channel.clone(),
            });
            Ok(())
        }
    }

    fn test_dispatcher(target: Arc<TestTarget>) -> Arc<Dispatcher> {
        Arc::new(Dispatcher::with_idle_timeout(
            target,
            1,
            24_000,
            crate::dispatch::BatchGrouping::Thread,
            Duration::from_secs(1),
        ))
    }

    fn mock_adapter(platform: &'static str) -> Arc<MockAdapter> {
        Arc::new(MockAdapter {
            platform,
            ..MockAdapter::default()
        })
    }

    fn registration(adapter: Arc<MockAdapter>, dispatcher: Arc<Dispatcher>) -> HandoffRegistration {
        HandoffRegistration {
            adapter,
            dispatcher,
            parent_channel: ChannelRef {
                platform: "discord".into(),
                channel_id: "parent".into(),
                thread_id: None,
                parent_id: None,
                origin_event_id: None,
            },
            trigger_msg: MessageRef {
                channel: ChannelRef {
                    platform: "discord".into(),
                    channel_id: "parent".into(),
                    thread_id: None,
                    parent_id: None,
                    origin_event_id: None,
                },
                message_id: "msg".into(),
            },
            sender_json: "{}".into(),
            sender_name: "alice".into(),
            sender_id: "u1".into(),
            original_prompt: "original".into(),
            extra_blocks: Vec::new(),
            other_bot_present: false,
        }
    }

    #[tokio::test]
    async fn token_is_single_use() {
        let broker = HandoffBroker::new(Duration::from_secs(60));
        let target = Arc::new(TestTarget::default());
        let adapter = mock_adapter("discord");
        let token = broker
            .register(registration(adapter, test_dispatcher(target)))
            .await;

        assert!(broker.consume(&token).await.is_ok());
        let err = match broker.consume(&token).await {
            Ok(_) => panic!("second consume should fail"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("invalid") || err.contains("already used"));
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let broker = HandoffBroker::new(Duration::from_millis(1));
        let target = Arc::new(TestTarget::default());
        let adapter = mock_adapter("discord");
        let token = broker
            .register(registration(adapter, test_dispatcher(target)))
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        let err = match broker.consume(&token).await {
            Ok(_) => panic!("expired consume should fail"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("expired"));
    }

    #[tokio::test]
    async fn start_thread_task_anchors_original_message_and_dispatches_child_prompt() {
        let broker = HandoffBroker::new(Duration::from_secs(60));
        let target = Arc::new(TestTarget::default());
        let adapter = mock_adapter("discord");
        let token = broker
            .register(registration(
                adapter.clone(),
                test_dispatcher(target.clone()),
            ))
            .await;

        let started = broker
            .start_thread_task(&token, "Investigate a complex issue", "child task prompt")
            .await
            .expect("handoff should start");

        assert_eq!(started.status, "started");
        assert_eq!(started.channel_id, "thread");
        let calls = adapter.create_calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.channel_id, "parent");
        assert_eq!(calls[0].1.message_id, "msg");
        drop(calls);

        let captured = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(item) = target.captured.lock().await.first().cloned() {
                    return item;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child dispatch should be captured");

        assert_eq!(captured.session_key, "discord:thread");
        assert_eq!(captured.thread_channel.channel_id, "thread");
        let text = captured
            .content_blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("<openab_handoff_context>"));
        assert!(text.contains("original_message:\noriginal"));
        assert!(text.contains("child task prompt"));
    }

    #[tokio::test]
    async fn start_thread_task_uses_slack_thread_root_route() {
        let broker = HandoffBroker::new(Duration::from_secs(60));
        let target = Arc::new(TestTarget::default());
        let adapter = mock_adapter("slack");
        let token = broker
            .register(registration(adapter, test_dispatcher(target.clone())))
            .await;

        let started = broker
            .start_thread_task(&token, "Investigate", "child task prompt")
            .await
            .expect("handoff should start");

        assert_eq!(started.platform, "slack");
        assert_eq!(started.channel_id, "C1");
        assert_eq!(started.thread_id.as_deref(), Some("111.222"));

        let captured = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(item) = target.captured.lock().await.first().cloned() {
                    return item;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child dispatch should be captured");
        assert_eq!(captured.session_key, "slack:111.222");
        assert_eq!(
            captured.thread_channel.thread_id.as_deref(),
            Some("111.222")
        );
    }
}
