//! Original-byte HLS fallback for providers requiring non-browser headers.
//! Only URLs discovered in an authenticated room's playlist can be requested.
use super::{Service, equal_secret};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Semaphore;
use watch_protocol::Media;

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let b = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && b[0] != 0
                && b[0] < 224
                && !(b[0] == 100 && (64..=127).contains(&b[1]))
                && !(b[0] == 198 && (18..=19).contains(&b[1]))
                && !(b[0] == 192 && b[1] == 0 && b[2] == 0)
        }
        IpAddr::V6(ip) => {
            let b = ip.octets();
            b[0] & 0xe0 == 0x20 && !(b[0..4] == [0x20, 0x01, 0x0d, 0xb8])
        }
    }
}
fn valid_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.as_str().len() <= 16384
}
fn register(
    value: &str,
    base: Option<&url::Url>,
    room: &str,
    token: &str,
    assets: &mut HashMap<String, String>,
) -> Result<String, StatusCode> {
    let url = match base {
        Some(base) => base.join(value),
        None => url::Url::parse(value),
    }
    .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if !valid_url(&url) || assets.len() >= 50000 {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let id = format!("{:x}", Sha1::digest(url.as_str().as_bytes()));
    assets.insert(id.clone(), url.into());
    Ok(format!("/media/{room}/{id}?token={token}"))
}
fn rewrite(
    text: &str,
    base: Option<&url::Url>,
    room: &str,
    token: &str,
    assets: &mut HashMap<String, String>,
) -> Result<String, StatusCode> {
    if !text.starts_with("#EXTM3U") || text.len() > 2 * 1024 * 1024 {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let mut result = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with('#') {
            result.push_str(&register(line, base, room, token, assets)?);
        } else {
            let mut rest = line;
            while let Some((prefix, tail)) = rest.split_once("URI=\"") {
                let (uri, tail) = tail.split_once('"').ok_or(StatusCode::BAD_GATEWAY)?;
                result.push_str(prefix);
                result.push_str("URI=\"");
                result.push_str(&register(uri, base, room, token, assets)?);
                result.push('"');
                rest = tail;
            }
            result.push_str(rest);
        }
        result.push('\n');
    }
    Ok(result)
}
fn playlist_response(text: String) -> Response {
    (
        [
            ("content-type", "application/vnd.apple.mpegurl"),
            ("referrer-policy", "no-referrer"),
        ],
        text,
    )
        .into_response()
}
pub(super) async fn serve(
    State(service): State<Arc<Service>>,
    Path((room, asset)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    static REQUESTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = REQUESTS
        .get_or_init(|| Arc::new(Semaphore::new(48)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let token = query.get("token").ok_or(StatusCode::UNAUTHORIZED)?;
    let source = {
        let mut rooms = service.rooms.lock().await;
        let session = rooms
            .get_mut(&room)
            .filter(|r| equal_secret(&r.token, token))
            .ok_or(StatusCode::NOT_FOUND)?;
        let Some(Media::HlsPlaylist { playlist }) = &session.media else {
            return Err(StatusCode::NOT_FOUND);
        };
        if asset == "master" {
            let playlist = playlist.clone();
            return Ok(playlist_response(rewrite(
                &playlist,
                None,
                &room,
                token,
                &mut session.media_assets,
            )?));
        }
        session
            .media_assets
            .get(&asset)
            .cloned()
            .ok_or(StatusCode::NOT_FOUND)?
    };
    let url = url::Url::parse(&source).map_err(|_| StatusCode::BAD_GATEWAY)?;
    if !valid_url(&url) {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let host = url.host_str().unwrap();
    let addresses: Vec<SocketAddr> =
        tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, 443)))
            .await
            .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
            .map_err(|_| StatusCode::BAD_GATEWAY)?
            .collect();
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        return Err(StatusCode::BAD_GATEWAY);
    }
    // Pin the validated DNS results for this request. No redirects, environment
    // proxy, private addresses, incoming cookies, or incoming auth are accepted.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let mut request = client
        .get(url.clone())
        .header("user-agent", "Mozilla/5.0")
        .header("origin", "https://cinejoy.pk")
        .header("referer", "https://cinejoy.pk/");
    if let Some(range) = headers.get("range") {
        let range = range.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
        if range.len() > 80
            || !range.starts_with("bytes=")
            || !range[6..].bytes().all(|b| b.is_ascii_digit() || b == b'-')
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        request = request.header("range", range);
    }
    let response = request.send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if !response.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let status = response.status();
    let upstream = response.headers().clone();
    let mut stream = response.bytes_stream();
    let first = stream
        .next()
        .await
        .ok_or(StatusCode::BAD_GATEWAY)?
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let is_playlist = upstream
        .get("content-type")
        .is_some_and(|h| h.to_str().unwrap_or("").contains("mpegurl"))
        || first.starts_with(b"#EXTM3U");
    if is_playlist {
        let mut bytes = first.to_vec();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_GATEWAY)?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(StatusCode::BAD_GATEWAY);
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
        let mut rooms = service.rooms.lock().await;
        let session = rooms.get_mut(&room).ok_or(StatusCode::GONE)?;
        return Ok(playlist_response(rewrite(
            text,
            Some(&url),
            &room,
            token,
            &mut session.media_assets,
        )?));
    }
    let stream = futures_util::stream::once(async move { Ok::<_, reqwest::Error>(first) })
        .chain(stream)
        .map(move |chunk| {
            let _held = &permit;
            chunk
        });
    let mut result = Response::builder().status(status);
    for name in [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
    ] {
        if let Some(value) = upstream.get(name) {
            result = result.header(name, value);
        }
    }
    result
        .body(Body::from_stream(stream))
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn media_requests_require_room_token_known_assets_and_public_addresses() {
        use super::super::{Access, Room, Viewer};
        let (host, _rx) = tokio::sync::mpsc::channel(4);
        let service = Arc::new(Service {
            connections: Arc::new(Semaphore::new(10)),
            rooms: tokio::sync::Mutex::default(),
            public_url: String::new(),
            key: String::new(),
            turn_urls: vec![],
            turn_secret: String::new(),
            max_rooms: 10,
            idle_since: std::sync::Mutex::new(std::time::Instant::now()),
            sleep_claim: std::sync::Mutex::new(None),
            idle_seconds: 900,
        });
        service.rooms.lock().await.insert("room".into(), Room {
            chat: crate::social::ChatHistory::default(),
            media_assets: HashMap::from([("private".into(), "https://169.254.169.254/metadata".into())]),
            media: Some(Media::HlsPlaylist { playlist: "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttps://cdn.example/video.m3u8\n".into() }),
            token: "secret".into(), relay: None, uplink_feedback: false, host,
            viewers: HashMap::<String, Viewer>::new(), state: None, access: Access::default(),
        });
        async fn request(
            service: Arc<Service>,
            asset: &str,
            token: &str,
        ) -> Result<Response, StatusCode> {
            serve(
                State(service),
                Path(("room".into(), asset.into())),
                Query(HashMap::from([("token".into(), token.into())])),
                HeaderMap::new(),
            )
            .await
        }
        assert!(matches!(
            request(service.clone(), "master", "wrong").await,
            Err(StatusCode::NOT_FOUND)
        ));
        assert_eq!(
            request(service.clone(), "master", "secret")
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(matches!(
            request(service.clone(), "unknown", "secret").await,
            Err(StatusCode::NOT_FOUND)
        ));
        assert!(matches!(
            request(service.clone(), "private", "secret").await,
            Err(StatusCode::BAD_GATEWAY)
        ));
        service.rooms.lock().await.remove("room");
        assert!(matches!(
            request(service, "master", "secret").await,
            Err(StatusCode::NOT_FOUND)
        ));
    }
    #[test]
    fn blocks_private_reserved_and_metadata_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.168.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
    #[test]
    fn rewrites_segments_keys_maps_and_relative_playlists() {
        let mut assets = HashMap::new();
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"../key\"\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2,\nsegment.ts\n";
        let base = url::Url::parse("https://cdn.example/movie/index.m3u8").unwrap();
        let rewritten = rewrite(text, Some(&base), "room", "secret", &mut assets).unwrap();
        assert_eq!(assets.len(), 3);
        assert!(assets.values().any(|v| v == "https://cdn.example/key"));
        assert!(
            assets
                .values()
                .any(|v| v == "https://cdn.example/movie/segment.ts")
        );
        assert!(!rewritten.contains("cdn.example"));
        assert!(rewritten.contains("URI=\"/media/room/"));
        assert!(rewrite("#EXTM3U\nfile:///etc/passwd\n", None, "r", "t", &mut assets).is_err());
    }
}
