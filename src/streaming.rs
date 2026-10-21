pub mod cinejoy;
pub mod movie_search;

// Most website extraction uses mpv's ytdl hook; Cinejoy has a direct adapter.
use std::path::{Path, PathBuf};

pub const DRM_MESSAGE: &str = "Netflix requires account sign-in and DRM playback in a supported browser or its official app. This player cannot play Netflix URLs. YouTube, supported websites, and unencrypted direct streams can play here.";

pub fn check_service(source: &str) -> Result<(), String> {
    if let Ok(url) = url::Url::parse(source)
        && let Some(host) = url.host_str()
        && (host == "netflix.com" || host.ends_with(".netflix.com"))
    {
        return Err(DRM_MESSAGE.into());
    }
    Ok(())
}

pub fn network_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    let source = if input.starts_with("www.") {
        format!("https://{input}")
    } else {
        input.to_owned()
    };
    let url = url::Url::parse(&source).map_err(|_| {
        "Enter a complete URL, such as https://www.youtube.com/watch?v=…".to_string()
    })?;
    if !matches!(
        url.scheme(),
        "http"
            | "https"
            | "rtsp"
            | "rtmp"
            | "rtmps"
            | "rtp"
            | "udp"
            | "srt"
            | "ftp"
            | "ftps"
            | "mms"
            | "tcp"
            | "tls"
    ) || url.host().is_none()
    {
        return Err("Enter an HTTP(S) video URL or a supported network stream URL.".into());
    }
    check_service(url.as_str())?;
    Ok(url.into())
}

/// Prefer streams a laptop can decode and buffer comfortably. Direct media is
/// never transcoded; selection uses website formats or HLS bitrate variants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    DataSaver,
    #[default]
    Balanced,
    Original,
}
impl Quality {
    pub const ALL: [Self; 3] = [Self::DataSaver, Self::Balanced, Self::Original];
    pub fn label(self) -> &'static str {
        match self {
            Self::DataSaver => "Data saver (720p)",
            Self::Balanced => "Balanced (1080p)",
            Self::Original => "Original quality",
        }
    }
    pub fn hls_bitrate(self) -> &'static str {
        match self {
            Self::DataSaver => "2500000",
            Self::Balanced => "6000000",
            Self::Original => "max",
        }
    }
    pub fn format(self) -> &'static str {
        match self {
            Self::DataSaver => {
                "bv*[height<=720][vcodec^=avc1]+ba/b[height<=720][vcodec^=avc1]/bv*[height<=720]+ba/b[height<=720]/best"
            }
            Self::Balanced => {
                "bv*[height<=1080][vcodec^=avc1]+ba/b[height<=1080][vcodec^=avc1]/bv*[height<=1080]+ba/b[height<=1080]/best"
            }
            Self::Original => "bv*+ba/best",
        }
    }
}

pub fn is_network(source: &str) -> bool {
    url::Url::parse(source).is_ok_and(|url| url.host().is_some())
}
/// Send web pages straight to the extractor, rather than probing their HTML as
/// media first. Keep direct streams on mpv's fast native path.
pub fn is_webpage(source: &str) -> bool {
    let Ok(url) = url::Url::parse(source) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let extension = url
        .path()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    !matches!(
        extension.as_str(),
        "mp4"
            | "m4v"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "mpg"
            | "mpeg"
            | "ts"
            | "m2ts"
            | "m3u8"
            | "mpd"
            | "mp3"
            | "m4a"
            | "aac"
            | "flac"
            | "ogg"
            | "opus"
            | "wav"
            | "flv"
            | "m3u"
            | "pls"
    )
}
pub fn playback_source(source: &str, extractor_available: bool) -> String {
    if extractor_available && is_webpage(source) {
        format!("ytdl://{source}")
    } else {
        source.into()
    }
}
pub fn playback_error(
    source: &str,
    extractor_available: bool,
    summary: &str,
    details: &str,
) -> String {
    let unsupported_page = is_webpage(source)
        && extractor_available
        && details.to_ascii_lowercase().contains("unsupported url");
    let host = url::Url::parse(source)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "this website".into());
    let unsupported_hint = format!(
        "nvplay's website extractor does not support this page on {host}. It could not resolve a playable stream. Use a direct MP4/HLS media URL, or play the page in a browser. JavaScript players may need a site-specific extractor."
    );
    let hint = if !is_network(source) {
        ""
    } else if is_webpage(source) && !extractor_available {
        "Website playback needs yt-dlp. Install it, then restart nvplay. A direct video or stream URL can play without it."
    } else if unsupported_page {
        &unsupported_hint
    } else if summary.to_ascii_lowercase().contains("format")
        || (is_webpage(source) && summary.to_ascii_lowercase().contains("loading failed"))
    {
        "The URL did not resolve to playable media. The site may require an embedded video's own URL, authentication, or a newer yt-dlp. See the extractor details below."
    } else {
        ""
    };
    // mpv tries the synthetic ytdl:// URL after the hook fails. These are
    // secondary errors, not a missing HTTP/media protocol in the engine.
    let details = details
        .lines()
        .filter(|line| {
            !unsupported_page
                || !(line.contains("No protocol handler found to open URL ytdl://")
                    || line.contains("The protocol is either unsupported, or was disabled")
                    || line.contains("youtube-dl failed: unexpected error occurred"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let summary = if unsupported_page {
        "Website not supported"
    } else {
        summary
    };
    [summary, hint, &details]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub struct Tools {
    pub ytdlp: Option<PathBuf>,
    pub deno: Option<PathBuf>,
}
impl Tools {
    pub fn discover() -> Self {
        Self {
            ytdlp: find_tool("NVPLAY_YTDLP", "yt-dlp"),
            deno: find_tool("NVPLAY_DENO", "deno"),
        }
    }
}

pub(crate) fn find_tool(variable: &str, name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(variable) {
        return executable(&PathBuf::from(path));
    }
    let mut directories = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        directories.push(parent.to_path_buf());
        // A macOS bundle can carry tools in Contents/MacOS or Contents/Resources.
        if let Some(contents) = parent.parent() {
            directories.push(contents.join("Resources"));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    #[cfg(target_os = "macos")]
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    for directory in directories {
        #[cfg(target_os = "windows")]
        if let Some(path) = executable(&directory.join(format!("{name}.exe"))) {
            return Some(path);
        }
        if let Some(path) = executable(&directory.join(name)) {
            return Some(path);
        }
    }
    None
}

fn executable(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    path.canonicalize().ok()
}

/// mpv's fixed-length quoting preserves commas, equals signs, backslashes,
/// and non-ASCII characters in key/value options without shell interpolation.
pub fn option_value(key: &str, value: &str) -> String {
    format!("{key}=%{}%{value}", value.len())
}

/// Helpers stay in the background on Windows as well as macOS.
pub(crate) fn tool_command(path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    // The binding is mutable only on Windows.
    let _ = &mut command;
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_links_and_service_limits() {
        assert_eq!(
            network_url(" www.youtube.com/watch?v=test ").unwrap(),
            "https://www.youtube.com/watch?v=test"
        );
        assert!(network_url("https://example.com/live.m3u8?token=a&quality=high").is_ok());
        assert!(network_url("rtsp://127.0.0.1:8554/live").is_ok());
        assert!(network_url("udp://239.1.1.1:1234").is_ok());
        assert!(network_url("this isn't a URL").is_err());
        assert!(network_url("javascript:alert(1)").is_err());
        assert!(network_url("file:///etc/passwd").is_err());
        assert!(
            network_url("https://www.netflix.com/watch/123")
                .unwrap_err()
                .contains("DRM")
        );
        assert!(check_service("https://netflix.com/watch/123").is_err());
        assert!(check_service("https://netflix.com.example.com/video.mp4").is_ok());
        assert!(check_service("/movies/netflix.com.mp4").is_ok());
    }
    #[test]
    fn webpage_extraction_preserves_direct_stream_fast_path() {
        assert_eq!(
            playback_source("https://example.com/watch?id=1", true),
            "ytdl://https://example.com/watch?id=1"
        );
        for source in [
            "https://example.com/VIDEO.MP4?token=x",
            "https://example.com/live.m3u8",
            "https://example.com/video.mpd",
            "rtsp://example.com/live",
            "/tmp/movie.mp4",
        ] {
            assert_eq!(playback_source(source, true), source);
        }
        assert_eq!(
            playback_source("https://example.com/download?id=1", false),
            "https://example.com/download?id=1"
        );
        assert!(
            playback_error(
                "https://example.com/watch",
                true,
                "unrecognized file format",
                "Unsupported URL"
            )
            .contains("does not support this page")
        );
        assert!(
            playback_error(
                "https://example.com/watch",
                false,
                "unrecognized file format",
                ""
            )
            .contains("Install it")
        );
        assert_eq!(
            playback_error("/tmp/bad.mp4", true, "unrecognized file format", ""),
            "unrecognized file format"
        );
    }

    #[test]
    fn unsupported_website_preserves_cause_without_synthetic_protocol_errors() {
        let details = "ytdl_hook: ERROR: Unsupported URL: https://cinejoy.pk/watch/movie/210577\nytdl_hook: youtube-dl failed: unexpected error occurred\nstream: No protocol handler found to open URL ytdl://https://cinejoy.pk/watch/movie/210577\nstream: The protocol is either unsupported, or was disabled at compile-time.";
        let error = playback_error(
            "https://cinejoy.pk/watch/movie/210577",
            true,
            "loading failed",
            details,
        );
        assert!(error.starts_with("Website not supported"));
        assert!(error.contains("on cinejoy.pk"));
        assert!(error.contains("ERROR: Unsupported URL:"));
        assert!(!error.contains("ytdl://"));
        assert!(!error.contains("disabled at compile-time"));
        assert!(!error.contains("Try the video's own page"));
        let protocol_error =
            "stream: The protocol is either unsupported, or was disabled at compile-time.";
        assert!(
            playback_error(
                "rtsp://example.com/video",
                true,
                "loading failed",
                protocol_error
            )
            .contains(protocol_error)
        );
    }

    #[test]
    #[ignore = "requires yt-dlp"]
    fn real_quality_selection_avoids_unnecessary_4k() {
        use std::process::Command;
        let tool = find_tool("NVPLAY_YTDLP", "yt-dlp").unwrap();
        let path = std::env::temp_dir().join(format!("nvplay-formats-{}.json", std::process::id()));
        std::fs::write(&path, r#"{
            "id":"fixture","title":"fixture","extractor":"generic","webpage_url":"https://example.com/watch",
            "formats":[
                {"format_id":"720","url":"https://example.com/720.mp4","height":720,"vcodec":"avc1.64001f","acodec":"none","ext":"mp4"},
                {"format_id":"1080","url":"https://example.com/1080.mp4","height":1080,"vcodec":"avc1.640028","acodec":"none","ext":"mp4"},
                {"format_id":"2160","url":"https://example.com/2160.webm","height":2160,"vcodec":"av01.0.12M.08","acodec":"none","ext":"webm"},
                {"format_id":"audio","url":"https://example.com/audio.m4a","vcodec":"none","acodec":"mp4a.40.2","ext":"m4a"}
            ]
        }"#).unwrap();
        for (quality, expected) in [
            (Quality::DataSaver, "720+audio"),
            (Quality::Balanced, "1080+audio"),
            (Quality::Original, "2160+audio"),
        ] {
            let output = Command::new(&tool)
                .args(["--ignore-config", "--load-info-json"])
                .arg(&path)
                .args(["--simulate", "--print", "format_id", "-f", quality.format()])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn option_quoting_counts_utf8_bytes() {
        let path = "C:\\Vidéo, tools\\yt-dlp.exe";
        assert_eq!(
            option_value("path", path),
            format!("path=%{}%{}", path.len(), path)
        );
    }
}
