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

/// Browser-owned playback. No media bytes pass through the host or relay.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Media {
    Youtube { id: String },
    Url { url: String },
    HlsPlaylist { playlist: String },
}
impl Media {
    pub fn valid(&self) -> bool {
        fn web_url(value: &str) -> bool {
            value.len() <= 16384
                && url::Url::parse(value).is_ok_and(|u| {
                    matches!(u.scheme(), "http" | "https")
                        && u.host_str().is_some()
                        && u.username().is_empty()
                        && u.password().is_none()
                })
        }
        match self {
            Self::Youtube { id } => {
                id.len() == 11
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            }
            Self::Url { url } => web_url(url),
            Self::HlsPlaylist { playlist } => {
                // Only the resolver's small master playlist with absolute URLs.
                playlist.len() <= 16384
                    && playlist.starts_with("#EXTM3U\n")
                    && playlist.contains("#EXT-X-STREAM-INF:")
                    && playlist
                        .lines()
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .all(web_url)
                    && playlist.lines().all(|line| {
                        let mut rest = line;
                        while let Some((_, tail)) = rest.split_once("URI=") {
                            let Some(tail) = tail.strip_prefix('"') else {
                                return false;
                            };
                            let Some((uri, tail)) = tail.split_once('"') else {
                                return false;
                            };
                            if !web_url(uri) {
                                return false;
                            }
                            rest = tail;
                        }
                        true
                    })
            }
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<Media>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<Media>,
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
    fn browser_media_rejects_unsafe_sources() {
        assert!(
            Media::Youtube {
                id: "dQw4w9WgXcQ".into()
            }
            .valid()
        );
        assert!(
            !Media::Youtube {
                id: "../bad".into()
            }
            .valid()
        );
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:secret@example.com/v.mp4",
        ] {
            assert!(!Media::Url { url: url.into() }.valid());
        }
        let playlist = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"https://cdn.example/audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttps://cdn.example/video.m3u8\n";
        assert!(
            Media::HlsPlaylist {
                playlist: playlist.into()
            }
            .valid()
        );
        for invalid in [
            playlist.replace("https://cdn.example/audio.m3u8", "file:///etc/passwd"),
            playlist.replace("https://cdn.example/video.m3u8", "relative.m3u8"),
        ] {
            assert!(!Media::HlsPlaylist { playlist: invalid }.valid());
        }
        // Old clients can still create broadcast sessions.
        assert!(matches!(
            serde_json::from_str::<Message>(r#"{"type":"create","key":""}"#).unwrap(),
            Message::Create { media: None, .. }
        ));
    }
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
