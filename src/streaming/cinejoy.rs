//! Browser-free Cinejoy movie resolution in a permission-limited Deno process.
use super::{Quality, find_tool};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn movie_id(source: &str) -> Option<String> {
    let url = url::Url::parse(source).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(
            url.host_str()?,
            "cinejoy.pk" | "www.cinejoy.pk" | "cinejoy.to" | "www.cinejoy.to"
        )
    {
        return None;
    }
    let id = url
        .path()
        .trim_end_matches('/')
        .strip_prefix("/watch/movie/")?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_owned())
}

pub struct Playlist(PathBuf);
impl Playlist {
    pub fn path(&self) -> &str {
        self.0.to_str().expect("generated Unicode playlist path")
    }
}
impl Drop for Playlist {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub struct Resolver {
    child: Child,
    started: Instant,
}
impl Resolver {
    pub fn start(source: &str, quality: Quality) -> Result<Self, String> {
        Self::spawn(source, quality, false)
    }
    pub fn check(source: &str) -> Result<Self, String> {
        Self::spawn(source, Quality::Balanced, true)
    }
    fn spawn(source: &str, quality: Quality, check: bool) -> Result<Self, String> {
        let deno = find_tool("NVPLAY_DENO", "deno")
            .ok_or("Cinejoy playback requires Deno. Install it with brew install deno, then restart nvplay.")?;
        let height = match quality {
            Quality::DataSaver => "720",
            Quality::Balanced => "1080",
            Quality::Original => "99999",
        };
        let mut child = super::tool_command(&deno)
            .args([
                "run",
                "--no-prompt",
                "--quiet",
                "--allow-net",
                "-",
                source,
                height,
                if check { "check" } else { "play" },
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start the Cinejoy resolver: {e}"))?;
        if let Err(error) = child
            .stdin
            .take()
            .unwrap()
            .write_all(include_bytes!("cinejoy.ts"))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Could not initialize the Cinejoy resolver: {error}"
            ));
        }
        Ok(Self {
            child,
            started: Instant::now(),
        })
    }
    pub fn poll(&mut self) -> Option<Result<Playlist, String>> {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status,
            Ok(None) if self.started.elapsed() < Duration::from_secs(35) => return None,
            Ok(None) => {
                return Some(Err(
                    "Cinejoy did not respond within 35 seconds. Try again later.".into(),
                ));
            }
            Err(e) => return Some(Err(format!("Cinejoy resolver failed: {e}"))),
        };
        let mut output = String::new();
        if status.success() {
            let result = (|| {
                self.child
                    .stdout
                    .take()
                    .unwrap()
                    .take(32768)
                    .read_to_string(&mut output)
                    .map_err(|e| e.to_string())?;
                let json: serde_json::Value = serde_json::from_str(&output)
                    .map_err(|e| format!("Invalid Cinejoy response: {e}"))?;
                let text = json["playlist"]
                    .as_str()
                    .filter(|s| s.starts_with("#EXTM3U\n") && s.len() <= 16384)
                    .ok_or("Cinejoy returned an invalid playlist")?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!(
                    "nvplay-cinejoy-{}-{stamp}.m3u8",
                    std::process::id()
                ));
                let playlist = Playlist(path);
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&playlist.0)
                    .map_err(|e| e.to_string())?;
                file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
                Ok(playlist)
            })();
            Some(result)
        } else {
            let _ = self
                .child
                .stderr
                .take()
                .unwrap()
                .take(4096)
                .read_to_string(&mut output);
            Some(Err(format!(
                "Cinejoy could not resolve this movie.\n\n{}",
                output.trim()
            )))
        }
    }
}
impl Drop for Resolver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cinejoy_movie_routing_uses_exact_hosts_and_numeric_ids() {
        for url in [
            "https://cinejoy.pk/watch/movie/210577",
            "https://www.cinejoy.to/watch/movie/210577/?x=1",
        ] {
            assert_eq!(movie_id(url).as_deref(), Some("210577"));
        }
        for url in [
            "https://cinejoy.pk.evil.com/watch/movie/1",
            "https://example.com/watch/movie/1",
            "https://cinejoy.pk/watch/movie/",
            "https://cinejoy.pk/watch/tv/1/1/1",
            "https://cinejoy.pk/watch/movie/1/other",
            "https://cinejoy.pk/watch/movie/NaN",
        ] {
            assert!(movie_id(url).is_none(), "{url}");
        }
    }
}
