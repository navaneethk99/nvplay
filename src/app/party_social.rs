//! Session-only participants, chat, and join notifications.
use super::*;
use std::collections::{BTreeMap, VecDeque};
use watch_protocol::{ChatEntry, MAX_CHAT_HISTORY, Participant};

#[derive(Default)]
pub(super) struct Social {
    pub participants: BTreeMap<String, Participant>,
    messages: VecDeque<ChatEntry>,
    draft: String,
    pending: bool,
    error: Option<String>,
    pub open: bool,
    unread: usize,
    notices: VecDeque<(String, Instant)>,
}
impl Social {
    #[cfg(test)]
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"participants": self.participants.values().collect::<Vec<_>>(), "messages": self.messages, "notices": self.notices.iter().map(|(text, _)| text).collect::<Vec<_>>()})
    }

    pub fn joined(&mut self, peer: String, name: String) {
        let name = watch_protocol::display_name(&name).unwrap_or_else(|_| "Viewer".into());
        if self
            .participants
            .insert(
                peer.clone(),
                Participant {
                    peer,
                    name: name.clone(),
                    host: false,
                },
            )
            .is_none()
        {
            self.notices.push_back((
                format!("{name} joined the watch party"),
                Instant::now() + Duration::from_secs(4),
            ));
            while self.notices.len() > watch_protocol::MAX_VIEWERS {
                self.notices.pop_front();
            }
        }
    }
    pub fn left(&mut self, peer: &str) {
        self.participants.remove(peer);
    }
    pub fn received(&mut self, entry: ChatEntry) {
        if self.messages.iter().any(|m| m.id == entry.id) {
            return;
        }
        if entry.sender.host {
            self.pending = false;
            if self.draft.trim() == entry.text {
                self.draft.clear();
            }
            self.error = None;
        } else if !self.open {
            self.unread += 1;
        }
        self.messages.push_back(entry);
        while self.messages.len() > MAX_CHAT_HISTORY {
            self.messages.pop_front();
        }
    }
    pub fn rejected(&mut self, error: String) {
        self.pending = false;
        self.error = Some(error);
    }
}

impl PlayerApp {
    pub(super) fn party_participants_ui(&mut self, ui: &mut egui::Ui) {
        ui.collapsing(
            format!(
                "Participants ({})",
                self.watch_party.social.participants.len() + 1
            ),
            |ui| {
                ui.label("Host (you)");
                for viewer in self.watch_party.social.participants.values() {
                    ui.label(&viewer.name);
                }
            },
        );
        let label = if self.watch_party.social.unread == 0 {
            "Open chat".into()
        } else {
            format!("Open chat ({})", self.watch_party.social.unread)
        };
        if ui.button(label).clicked() {
            self.watch_party.social.open = true;
        }
    }
    pub(super) fn watch_social_ui(&mut self, ctx: &egui::Context) {
        if self.watch_party.social.open {
            self.watch_party.social.unread = 0;
            let mut open = true;
            let mut send = false;
            egui::Window::new("Watch party chat")
                .open(&mut open)
                .collapsible(false)
                .default_width(340.0)
                .min_width(240.0)
                .max_width((ctx.content_rect().width() - 32.0).max(240.0))
                .show(ctx, |ui| {
                    ui.small(format!(
                        "Host + {} viewers · Messages stay in this session",
                        self.watch_party.social.participants.len()
                    ));
                    egui::ScrollArea::vertical()
                        .id_salt("party_chat_messages")
                        .max_height((ctx.content_rect().height() - 180.0).clamp(70.0, 250.0))
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            if self.watch_party.social.messages.is_empty() {
                                ui.label("Say hello to everyone watching.");
                            }
                            for message in &self.watch_party.social.messages {
                                ui.label(
                                    RichText::new(if message.sender.host {
                                        "Host (you)"
                                    } else {
                                        &message.sender.name
                                    })
                                    .strong()
                                    .color(ui.visuals().selection.stroke.color),
                                );
                                ui.label(&message.text);
                                ui.add_space(5.0);
                            }
                        });
                    ui.separator();
                    let active = self.watch_party.active();
                    let social = &mut self.watch_party.social;
                    ui.add_enabled_ui(active && !social.pending, |ui| {
                        let input = ui.add(
                            egui::TextEdit::singleline(&mut social.draft)
                                .hint_text("Message everyone…")
                                .char_limit(watch_protocol::MAX_CHAT_CHARS)
                                .desired_width(f32::INFINITY),
                        );
                        let valid = watch_protocol::chat_text(&social.draft).is_ok();
                        let enter = (input.has_focus() || input.lost_focus())
                            && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.horizontal(|ui| {
                            ui.small(format!(
                                "{}/{}",
                                social.draft.chars().count(),
                                watch_protocol::MAX_CHAT_CHARS
                            ));
                            if ui.add_enabled(valid, egui::Button::new("Send")).clicked()
                                || (enter && valid)
                            {
                                send = true;
                            }
                        });
                    });
                    if social.pending {
                        ui.small("Sending…");
                    }
                    if let Some(error) = &social.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                });
            self.watch_party.social.open = open;
            if send && let Some(host) = &self.watch_party.host {
                match host.send_chat(&self.watch_party.social.draft) {
                    Ok(()) => {
                        self.watch_party.social.pending = true;
                        self.watch_party.social.error = None;
                    }
                    Err(error) => self.watch_party.social.error = Some(error),
                }
            }
        }
        let now = Instant::now();
        self.watch_party
            .social
            .notices
            .retain(|(_, until)| *until > now);
        if !self.watch_party.social.notices.is_empty() {
            egui::Area::new(egui::Id::new("party_join_notifications"))
                .anchor(egui::Align2::RIGHT_TOP, [-12.0, 42.0])
                .order(egui::Order::Tooltip)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 24.0).min(320.0));
                    for (text, until) in &self.watch_party.social.notices {
                        egui::Frame::popup(ui.style())
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.label(text);
                            });
                        ctx.request_repaint_after(until.saturating_duration_since(now));
                        ui.add_space(4.0);
                    }
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_joins_do_not_repeat_notices_and_chat_waits_for_echo() {
        let mut social = Social::default();
        social.joined("peer".into(), "Alex".into());
        social.joined("peer".into(), "Alex".into());
        assert_eq!(social.notices.len(), 1);
        social.draft = "Hello".into();
        social.pending = true;
        social.received(ChatEntry {
            id: 1,
            sender: Participant {
                peer: "host".into(),
                name: "Host".into(),
                host: true,
            },
            text: "Hello".into(),
            sent_at: 0,
        });
        assert!(!social.pending && social.draft.is_empty());
        social.left("peer");
        assert!(social.participants.is_empty());
    }
}
