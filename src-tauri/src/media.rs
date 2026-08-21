//! Safe-ish public-media URL resolution for the browser video transmitter.
//!
//! Browsers may only read video pixels into a canvas when the media is same-origin
//! or explicitly CORS-enabled. This module creates short-lived opaque proxy URLs,
//! resolves standard page metadata, and optionally asks `yt-dlp` for provider pages.
//! Every network hop is limited to HTTP(S) and public IP space; redirects are
//! followed manually so they cannot hop into localhost or the lighting LAN.

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use parking_lot::Mutex;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, LOCATION, RANGE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

const MAX_HTML_BYTES: usize = 1_500_000;
const MAX_REDIRECTS: usize = 5;
const MAX_SESSIONS: usize = 128;
const SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const YTDLP_FORMAT: &str = "best[protocol^=http][vcodec!=none][acodec!=none][ext=mp4]/best[protocol^=http][vcodec!=none][acodec!=none]/best[protocol^=http][vcodec!=none][ext=mp4]/best[protocol^=http][vcodec!=none]";

#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub url: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveResponse {
    pub playback_url: String,
    pub title: String,
    pub source_url: String,
    pub resolved_by: String,
}

#[derive(Clone)]
struct MediaSession {
    url: Url,
    headers: HeaderMap,
    created: Instant,
}

pub struct MediaResolver {
    sessions: Mutex<HashMap<String, MediaSession>>,
}

impl MediaResolver {
    pub fn new() -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
        }))
    }

    pub async fn resolve(&self, input: &str) -> Result<ResolveResponse> {
        let source = Url::parse(input.trim()).context("That is not a valid URL")?;
        let _ = validate_public_url(&source).await?;

        // Instagram commonly puts even public post pages behind a login response,
        // while its extractor can still resolve the public media API. Try that
        // before making the generic page request so a 401/403 cannot short-circuit
        // extraction. Silent reels are valid inputs, too.
        let mut provider_error = None;
        let mut attempted_provider_url = None;
        if is_instagram_video_url(&source) {
            attempted_provider_url = Some(source.clone());
            match self
                .resolve_provider(&source, &source, "yt-dlp (Instagram)")
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => {
                    provider_error = Some(format!("the Instagram extractor failed ({error:#})"))
                }
            }
        }

        // Fast path: direct MP4/WebM/etc. URLs and servers declaring video content.
        let mut provider_url = source.clone();
        let page_error = match self
            .fetch_follow(source.clone(), HeaderMap::new(), None)
            .await
        {
            Ok((final_url, response)) => {
                provider_url = final_url.clone();
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if is_direct_media(&final_url, &content_type) {
                    return Ok(self.make_session(
                        final_url,
                        HeaderMap::new(),
                        title_from_url(&source),
                        source,
                        "direct",
                    ));
                }

                // Standard publisher pages often expose a playable file through Open
                // Graph or a literal <video>/<source> tag. Read a strictly bounded prefix.
                if content_type.contains("html") || content_type.is_empty() {
                    let html = read_prefix(response, MAX_HTML_BYTES).await?;
                    let html = String::from_utf8_lossy(&html);
                    let page_title = html_title(&html).unwrap_or_else(|| title_from_url(&source));
                    for candidate in html_video_candidates(&html, &final_url) {
                        if validate_public_url(&candidate).await.is_err() {
                            continue;
                        }
                        if let Ok((url, candidate_response)) =
                            self.fetch_follow(candidate, HeaderMap::new(), None).await
                        {
                            let ct = candidate_response
                                .headers()
                                .get(reqwest::header::CONTENT_TYPE)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            if is_direct_media(&url, &ct) {
                                return Ok(self.make_session(
                                    url,
                                    HeaderMap::new(),
                                    page_title,
                                    source,
                                    "page metadata",
                                ));
                            }
                        }
                    }
                }
                None
            }
            Err(error) => Some(format!("the page request failed ({error:#})")),
        };

        // Provider pages (YouTube, Vimeo, and many embeds) are intentionally an
        // optional capability: yt-dlp changes frequently and is not bundled into
        // the small standalone Gate binary. Prefer one muxed HTTP stream that the
        // browser can decode without ffmpeg, while accepting silent video (some
        // Instagram reels expose no audio track at all).
        if attempted_provider_url.as_ref() != Some(&provider_url) {
            let resolved_by = if is_instagram_video_url(&provider_url) {
                "yt-dlp (Instagram)"
            } else {
                "yt-dlp"
            };
            match self
                .resolve_provider(&provider_url, &source, resolved_by)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => {
                    provider_error = Some(format!("the provider extractor failed ({error:#})"))
                }
            }
        }

        let provider_error =
            provider_error.unwrap_or_else(|| "the provider extractor was unavailable".into());
        let page_error = page_error
            .map(|error| format!(" {error}."))
            .unwrap_or_default();

        bail!(
            "No directly playable video was found: {provider_error}.{page_error} Try another public URL, or update yt-dlp on the Gate machine."
        )
    }

    async fn resolve_provider(
        &self,
        provider_url: &Url,
        source: &Url,
        resolved_by: &str,
    ) -> Result<ResolveResponse> {
        let (url, title, headers, available_at) = resolve_with_ytdlp(provider_url.as_str()).await?;
        let _ = validate_public_url(&url).await?;
        // Some YouTube signatures deliberately become valid a few seconds after
        // extraction. yt-dlp exposes that instant as `available_at`; probing early
        // produces a misleading 403.
        if let Some(delay) = provider_delay(available_at) {
            tokio::time::sleep(delay).await;
        }
        // Extraction can still succeed while a provider rejects the signed URL
        // (expired tokens, missing Referer, etc.). Probe it now with the exact
        // headers yt-dlp supplied, which Instagram's CDN requires.
        let (url, response) = self
            .fetch_follow(url, headers.clone(), Some("bytes=0-0"))
            .await
            .context("the provider rejected its extracted stream")?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_direct_media(&url, &content_type) {
            bail!("the extracted stream was not browser-playable")
        }
        Ok(self.make_session(url, headers, title, source.clone(), resolved_by))
    }

    fn make_session(
        &self,
        url: Url,
        headers: HeaderMap,
        title: String,
        source: Url,
        resolved_by: &str,
    ) -> ResolveResponse {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut sessions = self.sessions.lock();
        sessions.retain(|_, s| s.created.elapsed() < SESSION_TTL);
        if sessions.len() >= MAX_SESSIONS
            && let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, session)| session.created)
                .map(|(id, _)| id.clone())
        {
            sessions.remove(&oldest);
        }
        sessions.insert(
            id.clone(),
            MediaSession {
                url,
                headers,
                created: Instant::now(),
            },
        );
        ResolveResponse {
            playback_url: format!("/media/stream/{id}"),
            title,
            source_url: source.to_string(),
            resolved_by: resolved_by.into(),
        }
    }

    pub async fn stream(&self, id: &str, range: Option<&str>) -> Result<reqwest::Response> {
        let session = self
            .sessions
            .lock()
            .get(id)
            .filter(|s| s.created.elapsed() < SESSION_TTL)
            .cloned()
            .ok_or_else(|| anyhow!("media link expired or unknown"))?;
        self.fetch_follow(session.url, session.headers, range)
            .await
            .map(|(_, r)| r)
    }

    async fn fetch_follow(
        &self,
        mut url: Url,
        headers: HeaderMap,
        range: Option<&str>,
    ) -> Result<(Url, reqwest::Response)> {
        for _ in 0..=MAX_REDIRECTS {
            let addresses = validate_public_url(&url).await?;
            let host = url.host_str().expect("validated URL has a host");
            // Pin this request to the exact public addresses we just inspected.
            // Without the override, a second DNS lookup inside reqwest creates a
            // rebinding window where the hostname could switch to a private IP.
            let client = reqwest::Client::builder()
                // A system HTTP proxy would perform its own DNS lookup and defeat
                // the validated-address pinning below.
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(8))
                .read_timeout(Duration::from_secs(20))
                .user_agent("EmpyreanGate/0.1 media resolver")
                .resolve_to_addrs(host, &addresses)
                .build()?;
            let mut request = client.get(url.clone()).headers(headers.clone());
            if let Some(range) = range {
                request = request.header(RANGE, range);
            }
            let response = request
                .send()
                .await
                .context("media server request failed")?;
            if !response.status().is_redirection() {
                if !response.status().is_success() {
                    bail!("media server returned {}", response.status());
                }
                return Ok((url, response));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("media redirect had no valid Location"))?;
            url = url.join(location).context("invalid media redirect")?;
        }
        bail!("too many media redirects")
    }
}

async fn read_prefix(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::with_capacity(limit.min(64 * 1024));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = limit.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if out.len() == limit {
            break;
        }
    }
    Ok(out)
}

fn is_direct_media(url: &Url, content_type: &str) -> bool {
    if content_type.starts_with("video/") {
        return true;
    }
    let path = url.path().to_ascii_lowercase();
    [".mp4", ".m4v", ".mov", ".webm", ".ogv"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn title_from_url(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut p| p.next_back())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.host_str().unwrap_or("Video"))
        .to_owned()
}

fn is_instagram_video_url(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host != "instagram.com" && host != "www.instagram.com" {
        return false;
    }
    let segments = url
        .path_segments()
        .map(|parts| parts.collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.first() == Some(&"reels") && segments.get(1) == Some(&"audio") {
        return false;
    }
    let is_video_segment = |segment: &&str| matches!(*segment, "p" | "reel" | "reels" | "tv");
    segments.first().is_some_and(is_video_segment) || segments.get(1).is_some_and(is_video_segment)
}

fn decode_html_attr(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn tag_attrs(tag: &str) -> HashMap<String, String> {
    let attr = Regex::new(r#"(?is)([a-z_:.-][a-z0-9_:.-]*)\s*=\s*["']([^"']*)["']"#)
        .expect("attribute regex");
    attr.captures_iter(tag)
        .map(|c| (c[1].to_ascii_lowercase(), decode_html_attr(&c[2])))
        .collect()
}

fn html_video_candidates(html: &str, base: &Url) -> Vec<Url> {
    let tags = Regex::new(r"(?is)<(?:meta|video|source)\b[^>]*>").expect("tag regex");
    let mut out = Vec::new();
    for tag in tags.find_iter(html) {
        let attrs = tag_attrs(tag.as_str());
        let key = attrs
            .get("property")
            .or_else(|| attrs.get("name"))
            .map(|v| v.to_ascii_lowercase());
        let raw = match key.as_deref() {
            Some("og:video")
            | Some("og:video:url")
            | Some("og:video:secure_url")
            | Some("twitter:player:stream") => attrs.get("content"),
            _ if tag.as_str().to_ascii_lowercase().starts_with("<video")
                || tag.as_str().to_ascii_lowercase().starts_with("<source") =>
            {
                attrs.get("src")
            }
            _ => None,
        };
        if let Some(raw) = raw
            && let Ok(url) = base.join(raw)
            && !out.contains(&url)
        {
            out.push(url);
        }
    }
    out
}

fn html_title(html: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<title[^>]*>\s*(.*?)\s*</title>").ok()?;
    let raw = re.captures(html)?.get(1)?.as_str();
    let title = decode_html_attr(raw)
        .trim()
        .chars()
        .take(160)
        .collect::<String>();
    (!title.is_empty()).then_some(title)
}

async fn resolve_with_ytdlp(input: &str) -> Result<(Url, String, HeaderMap, Option<u64>)> {
    let mut command = tokio::process::Command::new("yt-dlp");
    command.kill_on_drop(true).args([
        "--dump-single-json",
        "--no-playlist",
        "--skip-download",
        "--socket-timeout",
        "10",
        "--retries",
        "1",
        "--extractor-retries",
        "1",
        // Current YouTube extraction needs an external JS runtime. Node is
        // already used to build the UI and this remains harmless elsewhere.
        "--js-runtimes",
        "node",
        // Include the embedded client as a fallback: it still exposes a
        // browser-decodable muxed MP4 for many public/embeddable videos.
        "--extractor-args",
        "youtube:player_client=default,web_embedded",
        "--format",
        YTDLP_FORMAT,
        "--",
        input,
    ]);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .context("yt-dlp timed out")??;
    if !output.status.success() {
        bail!("yt-dlp could not resolve this page")
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let raw_url = json["url"]
        .as_str()
        .ok_or_else(|| anyhow!("yt-dlp returned no playable URL"))?;
    let url = Url::parse(raw_url)?;
    let title = json["title"].as_str().unwrap_or("Video").to_owned();
    let available_at = json["available_at"].as_u64();
    let mut headers = HeaderMap::new();
    if let Some(source_headers) = json["http_headers"].as_object() {
        for key in ["User-Agent", "Referer", "Origin"] {
            if let Some(value) = source_headers.get(key).and_then(|v| v.as_str())
                && let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(key.as_bytes()),
                    HeaderValue::from_str(value),
                )
            {
                headers.insert(name, value);
            }
        }
    }
    Ok((url, title, headers, available_at))
}

fn provider_delay(available_at: Option<u64>) -> Option<Duration> {
    let ready = available_at?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    (ready > now).then(|| Duration::from_secs((ready - now).min(15)))
}

async fn validate_public_url(url: &Url) -> Result<Vec<SocketAddr>> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http:// and https:// media URLs are allowed")
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("media URLs may not contain credentials")
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("media URL has no host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        bail!("local-network media URLs are blocked")
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("unknown media URL port"))?;
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .context("media host could not be resolved")?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        bail!("media URL resolves to a local, private, or reserved address")
    }
    Ok(addresses)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    // Public global-unicast space is currently 2000::/3. Deliberately excludes
    // loopback, link-local, ULA, multicast, and documentation ranges.
    let first = ip.octets()[0];
    let segments = ip.segments();
    (first & 0xe0) == 0x20 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_reserved_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn extracts_standard_video_metadata() {
        let base = Url::parse("https://example.com/watch/1").unwrap();
        let html = r#"
          <meta content="https://cdn.example.com/a.mp4?x=1&amp;y=2" property="og:video">
          <video src="/fallback.webm"></video>
        "#;
        let found = html_video_candidates(html, &base);
        assert_eq!(found[0].as_str(), "https://cdn.example.com/a.mp4?x=1&y=2");
        assert_eq!(found[1].as_str(), "https://example.com/fallback.webm");
    }

    #[test]
    fn recognizes_instagram_video_pages() {
        for input in [
            "https://www.instagram.com/reel/Chunk8-jurw/",
            "https://instagram.com/reels/Cop84x6u7CP/?utm_source=ig_web_copy_link",
            "https://www.instagram.com/p/aye83DjauH/",
            "https://www.instagram.com/tv/BkfuX9UB-eK/",
            "https://www.instagram.com/marvelskies.fc/reel/CWqAgUZgCku/",
        ] {
            assert!(
                is_instagram_video_url(&Url::parse(input).unwrap()),
                "{input}"
            );
        }
        for input in [
            "https://www.instagram.com/instagram/",
            "https://www.instagram.com/reels/audio/123/",
            "https://notinstagram.com/reel/Chunk8-jurw/",
        ] {
            assert!(
                !is_instagram_video_url(&Url::parse(input).unwrap()),
                "{input}"
            );
        }
    }

    #[test]
    fn provider_format_accepts_silent_instagram_reels() {
        assert!(YTDLP_FORMAT.contains("[vcodec!=none][ext=mp4]"));
        assert!(YTDLP_FORMAT.ends_with("best[protocol^=http][vcodec!=none]"));
    }

    #[test]
    fn proxy_sessions_have_a_hard_cap() {
        let resolver = MediaResolver::new().unwrap();
        for i in 0..MAX_SESSIONS + 5 {
            let url = Url::parse(&format!("https://example.com/{i}.mp4")).unwrap();
            resolver.make_session(
                url.clone(),
                HeaderMap::new(),
                format!("video {i}"),
                url,
                "test",
            );
        }
        assert_eq!(resolver.sessions.lock().len(), MAX_SESSIONS);
    }
}
