use super::*;
use std::collections::VecDeque;
use watch_protocol::{ChatEntry, HOST_PEER, MAX_CHAT_HISTORY, Participant, chat_text};

#[derive(Default)]
pub(super) struct ChatHistory {
    entries: VecDeque<ChatEntry>,
    sequence: u64,
}
impl ChatHistory {
    pub fn snapshot(&self) -> Vec<ChatEntry> {
        self.entries.iter().cloned().collect()
    }
    fn append(&mut self, sender: Participant, text: String) -> ChatEntry {
        self.sequence += 1;
        let entry = ChatEntry {
            id: self.sequence,
            sender,
            text,
            sent_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        self.entries.push_back(entry.clone());
        while self.entries.len() > MAX_CHAT_HISTORY {
            self.entries.pop_front();
        }
        entry
    }
}

pub(super) struct ChatRate {
    tokens: f64,
    updated: Instant,
}
impl Default for ChatRate {
    fn default() -> Self {
        Self {
            tokens: 5.0,
            updated: Instant::now(),
        }
    }
}
impl ChatRate {
    pub fn accept(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.updated).as_secs_f64() / 2.0).min(5.0);
        self.updated = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

impl Room {
    pub(super) fn participants(&self) -> Vec<Participant> {
        let mut result = vec![Participant {
            peer: HOST_PEER.into(),
            name: "Host".into(),
            host: true,
        }];
        let mut viewers: Vec<_> = self
            .viewers
            .iter()
            .map(|(peer, v)| Participant {
                peer: peer.clone(),
                name: v.name.clone(),
                host: false,
            })
            .collect();
        viewers.sort_by(|a, b| a.peer.cmp(&b.peer));
        result.extend(viewers);
        result
    }
    pub(super) fn broadcast(&self, message: Message) {
        let _ = self.host.try_send(message.clone());
        for viewer in self.viewers.values() {
            let _ = viewer.tx.try_send(message.clone());
        }
    }
    pub(super) fn chat_message(
        &mut self,
        peer: Option<&str>,
        text: &str,
    ) -> Result<Message, &'static str> {
        let sender = match peer {
            None => Participant {
                peer: HOST_PEER.into(),
                name: "Host".into(),
                host: true,
            },
            Some(id) => {
                let viewer = self
                    .viewers
                    .get(id)
                    .ok_or("You are no longer in this watch party.")?;
                Participant {
                    peer: id.into(),
                    name: viewer.name.clone(),
                    host: false,
                }
            }
        };
        let text = chat_text(text)?;
        Ok(Message::Chat {
            entry: self.chat.append(sender, text),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_history_and_rate_are_bounded() {
        let mut history = ChatHistory::default();
        let sender = Participant {
            peer: HOST_PEER.into(),
            name: "Host".into(),
            host: true,
        };
        for _ in 0..100 {
            history.append(sender.clone(), "🎬".repeat(watch_protocol::MAX_CHAT_CHARS));
        }
        assert_eq!(history.snapshot().len(), MAX_CHAT_HISTORY);
        assert_eq!(history.snapshot().last().unwrap().id, 100);
        let message = Message::Joined {
            participants: vec![sender],
            history: history.snapshot(),
            peer: "test".into(),
            media_proxy: None,
            media: None,
            client_offers: false,
            ice_servers: vec![],
            access: Access::default(),
        };
        assert!(serde_json::to_vec(&message).unwrap().len() < MAX_MESSAGE);
        let mut rate = ChatRate::default();
        for _ in 0..5 {
            assert!(rate.accept());
        }
        assert!(!rate.accept());
        rate.updated -= Duration::from_secs(2);
        assert!(rate.accept());
    }
}
