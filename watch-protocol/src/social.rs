//! Bounded, server-authored identities and chat shared by all clients.
use serde::{Deserialize, Serialize};

pub const MAX_NAME_CHARS: usize = 32;
pub const MAX_CHAT_CHARS: usize = 500;
pub const MAX_CHAT_HISTORY: usize = 40;
pub const HOST_PEER: &str = "host";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Participant {
    pub peer: String,
    pub name: String,
    pub host: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatEntry {
    pub id: u64,
    pub sender: Participant,
    pub text: String,
    pub sent_at: u64,
}

fn bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}
pub fn display_name(value: &str) -> Result<String, &'static str> {
    if value.len() > MAX_NAME_CHARS * 4 || value.chars().any(|c| c.is_control() || bidi_control(c))
    {
        return Err("Choose a name of 1–32 characters without control characters.");
    }
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() || value.chars().count() > MAX_NAME_CHARS {
        return Err("Choose a name of 1–32 characters.");
    }
    Ok(value)
}
pub fn chat_text(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_CHAT_CHARS * 4
        || value.chars().count() > MAX_CHAT_CHARS
    {
        return Err("Messages must contain 1–500 characters.");
    }
    if value
        .chars()
        .any(|c| (c.is_control() && !matches!(c, '\n' | '\t')) || bidi_control(c))
    {
        return Err("Messages cannot contain control characters.");
    }
    Ok(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_messages_are_bounded_unicode_plain_text() {
        assert_eq!(display_name("  Alex   Chen ").unwrap(), "Alex Chen");
        assert_eq!(display_name("Zoë 🎬").unwrap(), "Zoë 🎬");
        for name in ["", "  ", "A\nB", "\u{202e}Host"] {
            assert!(display_name(name).is_err());
        }
        assert!(display_name(&"é".repeat(33)).is_err());
        assert_eq!(
            chat_text(" <script>\nhello </script> ").unwrap(),
            "<script>\nhello </script>"
        );
        assert!(chat_text(&"🎬".repeat(500)).is_ok());
        assert!(chat_text(&"a".repeat(501)).is_err());
        assert!(chat_text("a\0b").is_err());
        assert!(chat_text("  ").is_err());
        assert!(
            serde_json::from_str::<crate::Message>(
                r#"{"type":"chat_send","text":"Hi","sender":{"peer":"host"}}"#
            )
            .is_err()
        );
    }
}
