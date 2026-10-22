//! Messages shared by the desktop broadcaster, signaling service, and browser.
use serde::{Deserialize, Serialize};

pub const MAX_VIEWERS: usize = 6;
pub const MAX_MESSAGE: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Playback {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub buffering: bool,
    pub speed: f64,
    pub title: String,
}
impl Playback {
    pub fn valid(&self) -> bool {
        self.position.is_finite()
            && self.position >= 0.0
            && self.duration.is_finite()
            && self.duration >= 0.0
            && self.speed.is_finite()
            && (0.25..=4.0).contains(&self.speed)
            && self.title.len() <= 1024
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Play {},
    Pause {},
    Seek { position: f64 },
}
impl Control {
    pub fn valid(&self) -> bool {
        match self {
            Self::Seek { position } => position.is_finite() && (0.0..=604800.0).contains(position),
            _ => true,
        }
    }
}

/// Host-controlled rights, applied to all current and future viewers.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Access {
    pub play_pause: bool,
    pub seek: bool,
    pub allow_join: bool,
}
impl Default for Access {
    fn default() -> Self {
        Self {
            play_pause: true,
            seek: true,
            allow_join: true,
        }
    }
}
impl Access {
    pub fn permits(&self, control: &Control) -> bool {
        match control {
            Control::Seek { .. } => self.seek,
            _ => self.play_pause,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credential: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Create {
        key: String,
        #[serde(default)]
        access: Access,
    },
    Created {
        url: String,
        ice_servers: Vec<IceServer>,
    },
    Join {
        room: String,
        token: String,
        #[serde(default)]
        client_offers: bool,
    },
    Joined {
        peer: String,
        #[serde(default)]
        client_offers: bool,
        ice_servers: Vec<IceServer>,
        #[serde(default)]
        access: Access,
    },
    PeerJoined {
        peer: String,
    },
    PeerLeft {
        peer: String,
    },
    Offer {
        peer: String,
        sdp: String,
    },
    Answer {
        peer: String,
        sdp: String,
    },
    Candidate {
        peer: String,
        candidate: serde_json::Value,
    },
    State {
        state: Playback,
    },
    /// Relay-observed video delivery and transport queue growth on the uplink.
    /// A host sends zero/zero before publishing to opt into these reports.
    Uplink {
        bitrate: u64,
        queue_ms: u64,
    },
    Control {
        control: Control,
    },
    Access {
        access: Access,
    },
    Denied {
        message: String,
    },
    Ended,
    Error {
        message: String,
    },
    Ping,
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_reject_invalid_targets_and_unknown_fields() {
        assert!(!Control::Seek { position: f64::NAN }.valid());
        assert!(!Control::Seek { position: -1.0 }.valid());
        assert!(Control::Seek { position: 123.4 }.valid());
        assert!(
            serde_json::from_str::<Control>(r#"{"action":"play","path":"/etc/passwd"}"#).is_err()
        );
    }
    #[test]
    fn shared_state_rejects_nonfinite_clocks() {
        let mut state = Playback {
            position: 0.0,
            duration: 100.0,
            paused: true,
            buffering: false,
            speed: 1.0,
            title: "Movie".into(),
        };
        assert!(state.valid());
        state.position = f64::INFINITY;
        assert!(!state.valid());
    }
}
