use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};

/// Message priority level within the swarm mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum MessagePriority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Structured asynchronous message routed between subagents and parent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailboxMessage {
    pub id: String,
    pub sender_id: String,
    pub recipient_id: String,
    pub payload: String,
    pub timestamp: DateTime<Utc>,
    pub priority: MessagePriority,
}

impl MailboxMessage {
    pub fn new(
        sender_id: impl Into<String>,
        recipient_id: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            sender_id: sender_id.into(),
            recipient_id: recipient_id.into(),
            payload: payload.into(),
            timestamp: Utc::now(),
            priority: MessagePriority::Normal,
        }
    }

    pub fn with_priority(mut self, priority: MessagePriority) -> Self {
        self.priority = priority;
        self
    }
}

/// Reactive events broadcast across the swarm event bus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MailboxEvent {
    MessageReceived(MailboxMessage),
    TaskCompleted { agent_id: String, result: String },
    TaskFailed { agent_id: String, error: String },
    StatusChanged { agent_id: String, status: String },
}

/// Asynchronous, reactive mailbox router facilitating inter-agent communication
/// and event-driven wakeups without busy-polling loops.
pub struct MailboxManager {
    inboxes: Arc<RwLock<HashMap<String, Vec<MailboxMessage>>>>,
    history: Arc<RwLock<Vec<MailboxMessage>>>,
    event_tx: broadcast::Sender<MailboxEvent>,
}

impl Default for MailboxManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MailboxManager {
    /// Initialize a new mailbox manager with a 1024-capacity broadcast channel.
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            inboxes: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
            event_tx,
        }
    }

    /// Send a message to a recipient agent, queuing in their inbox and broadcasting an event.
    pub async fn send_message(&self, message: MailboxMessage) -> anyhow::Result<()> {
        let recipient = message.recipient_id.clone();

        {
            let mut inboxes = self.inboxes.write().await;
            let queue = inboxes.entry(recipient).or_default();
            queue.push(message.clone());
            // Keep queue sorted by priority descending
            queue.sort_by_key(|a| std::cmp::Reverse(a.priority));
        }

        {
            let mut hist = self.history.write().await;
            hist.push(message.clone());
        }

        // Broadcast reactive event; ignore if there are no active receivers at the moment
        let _ = self.event_tx.send(MailboxEvent::MessageReceived(message));
        Ok(())
    }

    /// Retrieve and remove all pending messages for a given recipient.
    pub async fn receive_messages(&self, recipient_id: &str) -> Vec<MailboxMessage> {
        let mut inboxes = self.inboxes.write().await;
        inboxes.remove(recipient_id).unwrap_or_default()
    }

    /// Inspect pending messages for a recipient without removing them.
    pub async fn peek_messages(&self, recipient_id: &str) -> Vec<MailboxMessage> {
        let inboxes = self.inboxes.read().await;
        inboxes.get(recipient_id).cloned().unwrap_or_default()
    }

    /// Check if there are pending messages for a recipient.
    pub async fn has_messages(&self, recipient_id: &str) -> bool {
        let inboxes = self.inboxes.read().await;
        inboxes
            .get(recipient_id)
            .map(|q| !q.is_empty())
            .unwrap_or(false)
    }

    /// Clear the inbox of a specific recipient.
    pub async fn clear_inbox(&self, recipient_id: &str) {
        let mut inboxes = self.inboxes.write().await;
        inboxes.remove(recipient_id);
    }

    /// Retrieve full message history across all participants.
    pub async fn get_history(&self) -> Vec<MailboxMessage> {
        let hist = self.history.read().await;
        hist.clone()
    }

    /// Subscribe to reactive mailbox events.
    pub fn subscribe(&self) -> broadcast::Receiver<MailboxEvent> {
        self.event_tx.subscribe()
    }

    /// Broadcast a custom reactive lifecycle event (e.g. task completion or status change).
    pub fn notify_event(&self, event: MailboxEvent) -> anyhow::Result<()> {
        let _ = self.event_tx.send(event);
        Ok(())
    }

    /// Reactively wait until a message arrives for the specified recipient without busy-polling.
    /// Returns immediately if messages are already queued.
    pub async fn wait_for_message(
        &self,
        recipient_id: &str,
        timeout_dur: Option<Duration>,
    ) -> anyhow::Result<MailboxMessage> {
        // Subscribe to broadcast first to avoid any race condition
        let mut rx = self.event_tx.subscribe();

        // Check if message is already available in inbox
        {
            let mut inboxes = self.inboxes.write().await;
            if let Some(queue) = inboxes.get_mut(recipient_id) {
                if !queue.is_empty() {
                    return Ok(queue.remove(0));
                }
            }
        }

        let sleep_fut = async {
            if let Some(dur) = timeout_dur {
                tokio::time::sleep(dur).await;
                true
            } else {
                std::future::pending::<()>().await;
                false
            }
        };
        tokio::pin!(sleep_fut);

        loop {
            tokio::select! {
                timed_out = &mut sleep_fut => {
                    if timed_out {
                        anyhow::bail!("Timeout waiting for mailbox message for recipient '{}'", recipient_id);
                    }
                }
                event = rx.recv() => {
                    match event {
                        Ok(MailboxEvent::MessageReceived(msg)) if msg.recipient_id == recipient_id => {
                            let mut inboxes = self.inboxes.write().await;
                            if let Some(queue) = inboxes.get_mut(recipient_id) {
                                if let Some(idx) = queue.iter().position(|m| m.id == msg.id) {
                                    return Ok(queue.remove(idx));
                                }
                            }
                            return Ok(msg);
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let mut inboxes = self.inboxes.write().await;
                            if let Some(queue) = inboxes.get_mut(recipient_id) {
                                if !queue.is_empty() {
                                    return Ok(queue.remove(0));
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            anyhow::bail!("Mailbox broadcast channel closed");
                        }
                    }
                }
            }
        }
    }

    /// Reactively wait until a specific event condition is met without busy-polling.
    pub async fn wait_for_condition<F>(
        &self,
        filter: F,
        timeout_dur: Option<Duration>,
    ) -> anyhow::Result<MailboxEvent>
    where
        F: Fn(&MailboxEvent) -> bool,
    {
        let mut rx = self.event_tx.subscribe();

        let sleep_fut = async {
            if let Some(dur) = timeout_dur {
                tokio::time::sleep(dur).await;
                true
            } else {
                std::future::pending::<()>().await;
                false
            }
        };
        tokio::pin!(sleep_fut);

        loop {
            tokio::select! {
                timed_out = &mut sleep_fut => {
                    if timed_out {
                        anyhow::bail!("Timeout waiting for mailbox event condition");
                    }
                }
                event = rx.recv() => {
                    match event {
                        Ok(evt) if filter(&evt) => return Ok(evt),
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            anyhow::bail!("Mailbox broadcast channel closed");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mailbox_send_and_receive() -> anyhow::Result<()> {
        let mailbox = MailboxManager::new();

        let msg1 = MailboxMessage::new("agent_1", "parent", "hello parent")
            .with_priority(MessagePriority::Normal);
        let msg2 = MailboxMessage::new("agent_2", "parent", "critical alert")
            .with_priority(MessagePriority::Critical);

        mailbox.send_message(msg1).await?;
        mailbox.send_message(msg2).await?;

        assert!(mailbox.has_messages("parent").await);
        let messages = mailbox.receive_messages("parent").await;
        assert_eq!(messages.len(), 2);
        // Critical priority should come first
        assert_eq!(messages[0].priority, MessagePriority::Critical);
        assert_eq!(messages[0].payload, "critical alert");
        assert_eq!(messages[1].priority, MessagePriority::Normal);

        // Inbox should now be empty
        assert!(!mailbox.has_messages("parent").await);
        Ok(())
    }

    #[tokio::test]
    async fn test_mailbox_reactive_wait_without_polling() -> anyhow::Result<()> {
        let mailbox = Arc::new(MailboxManager::new());
        let mb_clone = Arc::clone(&mailbox);

        // Spawn sender with a tiny delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let msg =
                MailboxMessage::new("child_subagent", "parent_agent", "reactive notification");
            let _ = mb_clone.send_message(msg).await;
        });

        // Parent waits reactively without polling loop
        let start = std::time::Instant::now();
        let received = mailbox
            .wait_for_message("parent_agent", Some(Duration::from_secs(2)))
            .await?;
        let elapsed = start.elapsed();

        assert_eq!(received.payload, "reactive notification");
        assert_eq!(received.sender_id, "child_subagent");
        assert!(elapsed >= Duration::from_millis(40));
        assert!(elapsed < Duration::from_millis(500));
        Ok(())
    }

    #[tokio::test]
    async fn test_mailbox_timeout() {
        let mailbox = MailboxManager::new();
        let res = mailbox
            .wait_for_message("nonexistent", Some(Duration::from_millis(50)))
            .await;
        assert!(res.is_err());
    }
}
