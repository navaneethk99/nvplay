//! OpenSubtitles REST API. Network and disk work run outside the UI thread.
pub mod library;
pub mod settings;

use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const API: &str = "https://api.opensubtitles.com/api/v1";
const USER_AGENT: &str = concat!("nvplay v", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct Subtitle {
    pub file_id: u64,
    pub title: String,
    pub release: String,
    pub language: String,
    pub downloads: u64,
    pub hearing_impaired: bool,
}

pub fn search_url(query: &str, language: &str) -> String {
    let mut url = url::Url::parse(&format!("{API}/subtitles")).unwrap();
    url.query_pairs_mut()
        .append_pair("languages", &language.trim().to_lowercase())
        .append_pair("order_by", "download_count")
        .append_pair("order_direction", "desc")
        .append_pair("query", &query.trim().to_lowercase())
        .append_pair("type", "movie");
    url.into()
}

fn api(mut request: ehttp::Request, key: &str) -> Result<Value, String> {
    if key.trim().is_empty() {
        return Err("Enter an OpenSubtitles API key in Connection settings.".into());
    }
    request.headers.insert("Api-Key", key.trim());
    request.headers.insert("User-Agent", USER_AGENT);
    request.headers.insert("Accept", "application/json");
    request.headers.insert("Content-Type", "application/json");
    let response = ehttp::fetch_blocking(&request.with_timeout(Some(Duration::from_secs(20))))
        .map_err(|error| transport_error(&error, "OpenSubtitles"))?;
    let body = serde_json::from_slice::<Value>(&response.bytes).unwrap_or(Value::Null);
    if !response.ok {
        let reason = match response.status {
            401 | 403 => "OpenSubtitles rejected the API key. Check Connection settings.",
            406 => {
                "Your OpenSubtitles daily download allowance is exhausted. Try again after it resets."
            }
            429 => "OpenSubtitles is receiving too many requests. Wait a moment and try again.",
            _ => "OpenSubtitles could not complete the request. Try again later.",
        };
        return Err(format!("{reason} (HTTP {})", response.status));
    }
    if body.is_null() {
        return Err("OpenSubtitles returned an invalid response.".into());
    }
    Ok(body)
}

// Classify transport failures without exposing request headers, keys, or signed URLs.
fn transport_error(error: &str, service: &str) -> String {
    let error = error.to_ascii_lowercase();
    if error.contains("connection reset")
        || error.contains("unexpected eof")
        || error.contains("peer closed")
    {
        format!(
            "The connection to {service} was closed before a response arrived. Your network or the service may be blocking the connection. Try another network or a VPN, then retry."
        )
    } else if error.contains("certificate") || error.contains("cert") || error.contains("tls") {
        format!(
            "Could not establish a secure connection to {service}. Check your computer's date and trusted certificates, and any proxy or network filtering software."
        )
    } else if error.contains("dns") || error.contains("resolve") || error.contains("host not found")
    {
        format!(
            "Could not find {service}. Check your internet connection and DNS settings, then retry."
        )
    } else if error.contains("timed out") || error.contains("timeout") {
        format!("The connection to {service} timed out. Please retry, or try another network.")
    } else {
        format!(
            "Could not connect to {service}. Check your internet, proxy, or firewall settings and retry."
        )
    }
}

pub fn search(key: &str, query: &str, language: &str) -> Result<Vec<Subtitle>, String> {
    let body = api(ehttp::Request::get(search_url(query, language)), key)?;
    parse_results(&body)
}

fn parse_results(body: &Value) -> Result<Vec<Subtitle>, String> {
    let data = body["data"]
        .as_array()
        .ok_or("OpenSubtitles returned invalid search results.")?;
    let mut results = Vec::new();
    for item in data {
        let a = &item["attributes"];
        if let Some(files) = a["files"].as_array() {
            for file in files {
                if let Some(file_id) = file["file_id"].as_u64().filter(|id| *id > 0) {
                    results.push(Subtitle {
                        file_id,
                        title: a["feature_details"]["title"]
                            .as_str()
                            .unwrap_or("Unknown movie")
                            .into(),
                        release: a["release"]
                            .as_str()
                            .or(file["file_name"].as_str())
                            .unwrap_or("Unknown release")
                            .into(),
                        language: a["language"].as_str().unwrap_or("unknown").into(),
                        downloads: a["download_count"].as_u64().unwrap_or(0),
                        hearing_impaired: a["hearing_impaired"].as_bool().unwrap_or(false),
                    });
                }
            }
        }
    }
    Ok(results)
}

/// Own the downloaded file until the player exits; never trust server filenames.
#[derive(Debug)]
pub struct Downloaded {
    pub path: PathBuf,
    directory: PathBuf,
}
impl Drop for Downloaded {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Downloaded {
    pub(crate) fn save(bytes: &[u8]) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let directory = loop {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("nvplay-subtitle-{}-{sequence}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("Could not create a temporary subtitle folder.".into()),
            }
        };
        let file = Self {
            path: directory.join("subtitle.srt"),
            directory,
        };
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file.path)
            .and_then(|mut f| f.write_all(bytes))
            .map_err(|_| "Could not save the downloaded subtitle.".to_string())?;
        Ok(file)
    }
}

pub fn download(key: &str, file_id: u64) -> Result<Downloaded, String> {
    let body = api(
        ehttp::Request::post(
            format!("{API}/download"),
            json!({"file_id": file_id, "sub_format": "srt"})
                .to_string()
                .into_bytes(),
        ),
        key,
    )?;
    let link = body["link"]
        .as_str()
        .ok_or("OpenSubtitles did not provide a download link.")?;
    let url =
        url::Url::parse(link).map_err(|_| "OpenSubtitles returned an invalid download link.")?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("OpenSubtitles returned an insecure download link.".into());
    }
    // Do not forward API credentials to the file host.
    let mut request = ehttp::Request::get(url);
    request.headers.insert("User-Agent", USER_AGENT);
    let response = ehttp::fetch_blocking(&request.with_timeout(Some(Duration::from_secs(30))))
        .map_err(|error| transport_error(&error, "the subtitle download server"))?;
    validate_file(&response)?;
    Downloaded::save(&response.bytes)
}
fn validate_file(response: &ehttp::Response) -> Result<(), String> {
    if !response.ok || response.bytes.is_empty() || response.bytes.len() > 5 * 1024 * 1024 {
        return Err("The subtitle download was empty, too large, or unsuccessful.".into());
    }
    let text = String::from_utf8_lossy(&response.bytes);
    if !text.contains("-->") || text.trim_start().starts_with('<') {
        return Err("The downloaded file was not an SRT subtitle. Try another release.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires access to the OpenSubtitles API; no credentials are sent"]
    fn real_api_endpoint_is_reachable() {
        let request = ehttp::Request::get(format!("{API}/infos/languages"))
            .with_header("User-Agent", USER_AGENT)
            .with_timeout(Some(Duration::from_secs(15)));
        let response = ehttp::fetch_blocking(&request)
            .expect("OpenSubtitles transport failed before receiving an HTTP response");
        assert!(
            response.ok || matches!(response.status, 401 | 403 | 429),
            "Unexpected OpenSubtitles HTTP status: {}",
            response.status
        );
    }
    #[test]
    fn transport_errors_explain_recovery_without_exposing_credentials() {
        let reset = transport_error(
            "io: Connection reset by peer (os error 54)",
            "OpenSubtitles",
        );
        assert!(reset.contains("before a response"));
        assert!(reset.contains("another network"));
        assert!(
            transport_error("invalid certificate", "OpenSubtitles").contains("secure connection")
        );
        assert!(transport_error("dns resolution failed", "OpenSubtitles").contains("DNS"));
        assert!(transport_error("timeout", "OpenSubtitles").contains("timed out"));
        let message = transport_error("secret-key https://host/?signed=secret", "OpenSubtitles");
        assert!(!message.contains("secret"));
        assert!(!message.contains("signed"));
    }
    #[test]
    fn query_is_encoded_and_normalized() {
        let url = url::Url::parse(&search_url(" The Matrix & more ", "EN")).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs[0], ("languages".into(), "en".into()));
        assert_eq!(pairs[3], ("query".into(), "the matrix & more".into()));
        assert_eq!(pairs.len(), 5);
    }
    #[test]
    fn parses_all_files_and_skips_invalid_ids() {
        let results = parse_results(&json!({"data":[{"attributes":{
            "feature_details":{"title":"The Matrix"}, "language":"en", "release":"BluRay",
            "files":[{"file_id":12},{"file_id":13},{"file_id":0},{}]
        }}]}))
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].file_id, 13);
        assert_eq!(results[0].title, "The Matrix");
        assert!(parse_results(&json!({"data":[]})).unwrap().is_empty());
        assert!(parse_results(&json!({})).is_err());
    }
    #[test]
    fn temporary_subtitles_are_unique_and_cleaned_up() {
        let first = Downloaded::save(b"1\n00:00:01,000 --> 00:00:02,000\nHello\n").unwrap();
        let second = Downloaded::save(b"other subtitle").unwrap();
        assert_ne!(first.path, second.path);
        let directory = first.directory.clone();
        assert!(first.path.exists());
        drop(first);
        assert!(!directory.exists());
        assert!(second.path.exists());
    }
}
