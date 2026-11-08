//! Now-playing for the Spotify desktop app, controlled through AppleScript.
//! Quill talks to the app already running on this Mac — no account — and
//! checks that Spotify is running before addressing it, so it never launches
//! Spotify on its own.
//!
//! The one network request in Quill is here: the current track's cover art.
//! It is fetched only from Spotify's image CDN, over HTTPS with no redirects,
//! no cookies and a size cap, and nothing about the user or their documents
//! is sent. Offline or on any failure the player simply shows no art.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

const POLL_EVERY: Duration = Duration::from_secs(2);
const ART_TIMEOUT: Duration = Duration::from_secs(5);
const ART_MAX_BYTES: u64 = 2 * 1024 * 1024;
const ART_CACHE: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// 0.0..=1.0 through the track.
    pub progress: f32,
    pub artwork_url: Option<String>,
}

#[derive(Clone, Copy)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

/// App-wide now-playing state; views observe it.
pub struct Media {
    pub now: Option<NowPlaying>,
    /// Cover art by URL; `None` marks a URL that failed, so it isn't retried.
    art: HashMap<String, Option<Arc<Image>>>,
    _poll: Task<()>,
}

impl Media {
    pub fn new(cx: &mut Context<Self>) -> Media {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let now = cx.background_executor().spawn(async { now_playing() }).await;
                let alive = this
                    .update(cx, |media, cx| {
                        if media.now != now {
                            media.now = now;
                            media.fetch_art(cx);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(POLL_EVERY).await;
            }
        });
        Media { now: None, art: HashMap::new(), _poll: poll }
    }

    /// The current track's cover, once fetched.
    pub fn artwork(&self) -> Option<Arc<Image>> {
        let url = self.now.as_ref()?.artwork_url.as_ref()?;
        self.art.get(url).cloned().flatten()
    }

    fn fetch_art(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.now.as_ref().and_then(|n| n.artwork_url.clone()) else { return };
        if self.art.contains_key(&url) || !is_allowed_art_url(&url) {
            return;
        }
        if self.art.len() >= ART_CACHE {
            self.art.clear();
        }
        // Reserve the slot so the next poll doesn't start a second request.
        self.art.insert(url.clone(), None);
        let request = url.clone();
        let task = cx.background_executor().spawn(async move { download_art(&request) });
        cx.spawn(async move |this, cx| {
            let image = task.await;
            this.update(cx, |media, cx| {
                if let Some(image) = image {
                    media.art.insert(url, Some(Arc::new(image)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn control(&mut self, control: Control, cx: &mut Context<Self>) {
        if let Some(now) = &mut self.now
            && let Control::PlayPause = control
        {
            now.playing = !now.playing;
            cx.notify();
        }
        cx.background_executor()
            .spawn(async move {
                let verb = match control {
                    Control::PlayPause => "playpause",
                    Control::Next => "next track",
                    Control::Previous => "previous track",
                };
                run(&format!("if application \"Spotify\" is running then tell application \"Spotify\" to {verb}"));
            })
            .detach();
    }
}

const QUERY: &str = r#"
if application "Spotify" is running then
    tell application "Spotify"
        set s to player state as string
        if s is "stopped" then return ""
        set t to current track
        return s & tab & (name of t) & tab & (artist of t) & tab & (player position as string) & tab & ((duration of t) as string) & tab & (artwork url of t)
    end tell
end if
return ""
"#;

fn now_playing() -> Option<NowPlaying> {
    parse(&run(QUERY)?)
}

fn run(script: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("osascript").arg("-e").arg(script).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Parses `state \t name \t artist \t position-seconds \t duration-ms \t artwork-url`.
fn parse(line: &str) -> Option<NowPlaying> {
    let mut parts = line.split('\t');
    let state = parts.next()?;
    let title = parts.next()?.to_owned();
    let artist = parts.next()?.to_owned();
    // AppleScript formats reals with the user's locale decimal separator.
    let number = |s: Option<&str>| s.and_then(|s| s.trim().replace(',', ".").parse::<f32>().ok());
    let position = number(parts.next()).unwrap_or(0.);
    let duration_ms = number(parts.next()).unwrap_or(0.);
    let progress = if duration_ms > 0. { (position * 1000. / duration_ms).clamp(0., 1.) } else { 0. };
    let artwork_url = parts.next().map(str::trim).filter(|u| !u.is_empty() && *u != "missing value").map(String::from);
    Some(NowPlaying { title, artist, playing: state == "playing", progress, artwork_url })
}

/// Only Spotify's image CDN, over HTTPS, with no credentials or custom port.
fn is_allowed_art_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.contains(['@', ':']) {
        return false;
    }
    let host = host.to_ascii_lowercase();
    host == "i.scdn.co" || host.ends_with(".scdn.co")
}

fn download_art(url: &str) -> Option<Image> {
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(ART_TIMEOUT))
        .user_agent("Quill")
        .accept("image/jpeg, image/png")
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent.get(url).call().ok()?;
    let bytes = response.body_mut().with_config().limit(ART_MAX_BYTES).read_to_vec().ok()?;
    // Trust the bytes, not the headers.
    let format = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        ImageFormat::Jpeg
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else {
        return None;
    };
    Some(Image::from_bytes(format, bytes))
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_art_url, parse};

    #[test]
    fn parses_osascript_output() {
        let now =
            parse("playing\tGymnopédie No. 1\tErik Satie\t76,5\t204000\thttps://i.scdn.co/image/ab67616d0000b273")
                .unwrap();
        assert_eq!(now.title, "Gymnopédie No. 1");
        assert_eq!(now.artist, "Erik Satie");
        assert!(now.playing);
        assert!((now.progress - 0.375).abs() < 0.001);
        assert_eq!(now.artwork_url.as_deref(), Some("https://i.scdn.co/image/ab67616d0000b273"));
        let paused = parse("paused\tA\tB\t0\t0\tmissing value").unwrap();
        assert!(!paused.playing);
        assert_eq!(paused.artwork_url, None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn art_urls_are_limited_to_spotify_cdn_over_https() {
        assert!(is_allowed_art_url("https://i.scdn.co/image/ab67616d0000b273"));
        assert!(is_allowed_art_url("https://mosaic.scdn.co/640/abc"));
        assert!(!is_allowed_art_url("http://i.scdn.co/image/x"));
        assert!(!is_allowed_art_url("https://evil.com/i.scdn.co/x"));
        assert!(!is_allowed_art_url("https://i.scdn.co.evil.com/x"));
        assert!(!is_allowed_art_url("https://user@i.scdn.co/x"));
        assert!(!is_allowed_art_url("https://i.scdn.co:8443/x"));
        assert!(!is_allowed_art_url("https://scdn.co.attacker/x"));
        assert!(!is_allowed_art_url("file:///etc/passwd"));
    }
}
