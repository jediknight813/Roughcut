//! Local Spotify and Apple Music/iTunes playback through macOS automation.
//! Running players are queried without launching them. Artwork is fetched only
//! from Spotify's HTTPS image CDN; music and document data stay local.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui_kit::*;

const POLL_EVERY: Duration = Duration::from_secs(2);
const ART_TIMEOUT: Duration = Duration::from_secs(5);
const ART_MAX_BYTES: u64 = 2 * 1024 * 1024;
const ART_CACHE: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub player: Player,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// 0.0..=1.0 through the track.
    pub progress: f32,
    pub artwork_url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Player {
    Spotify,
    Music,
    ITunes,
}

impl Player {
    pub fn name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::Music => "Apple Music",
            Self::ITunes => "iTunes",
        }
    }
    fn application(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::Music => "Music",
            Self::ITunes => "iTunes",
        }
    }
    fn installed(self) -> bool {
        let app = format!("{}.app", self.application());
        [std::path::PathBuf::from("/Applications"), std::path::PathBuf::from("/System/Applications")]
            .into_iter()
            .chain(directories::BaseDirs::new().map(|d| d.home_dir().join("Applications")))
            .any(|dir| dir.join(&app).exists())
    }
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
    pub error: Option<String>,
    /// Cover art by URL; `None` marks a URL that failed, so it isn't retried.
    art: HashMap<String, Option<Arc<Image>>>,
    _poll: Task<()>,
}

impl Media {
    pub fn new(cx: &mut Context<Self>) -> Media {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let (now, error) = cx.background_executor().spawn(async { now_playing() }).await;
                let alive = this
                    .update(cx, |media, cx| {
                        if media.now != now || media.error != error {
                            media.error = error;
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
        Media { now: None, error: None, art: HashMap::new(), _poll: poll }
    }

    /// A player that never polls media apps (for tests and previews).
    pub fn idle() -> Media {
        Media { now: None, error: None, art: HashMap::new(), _poll: Task::ready(()) }
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
        let Some(player) = self.now.as_ref().map(|n| n.player) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let verb = match control {
                Control::PlayPause => "playpause",
                Control::Next => "next track",
                Control::Previous => "previous track",
            };
            let app = player.application();
            run(&format!("if application \"{app}\" is running then tell application \"{app}\" to {verb}"))
                .map_err(|error| player_error(player, &error))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let (now, poll_error) = cx.background_executor().spawn(async { now_playing() }).await;
            this.update(cx, |media, cx| {
                media.error = result.err().or(poll_error);
                media.now = now;
                media.fetch_art(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn query(player: Player) -> String {
    let app = player.application();
    let (duration, art) = if player == Player::Spotify {
        ("duration of t", "artwork url of t")
    } else {
        ("(duration of t) * 1000", "\"\"")
    };
    format!(
        r#"
with timeout of 5 seconds
    if application "{app}" is running then
        tell application "{app}"
            set s to player state as string
            if s is "stopped" then return ""
            set t to current track
            set cover to ""
            try
                set cover to {art}
            end try
            return s & tab & (name of t) & tab & (artist of t) & tab & (player position as string) & tab & (({duration}) as string) & tab & cover
        end tell
    end if
end timeout
return ""
"#
    )
}

fn player_error(player: Player, error: &str) -> String {
    if error.contains("-1743") || error.contains("not authorized") {
        format!("Allow Roughcut to control {} in System Settings → Privacy & Security → Automation.", player.name())
    } else {
        format!("{} connection failed. {}", player.name(), error.trim())
    }
}

fn choose_player(tracks: impl IntoIterator<Item = NowPlaying>) -> Option<NowPlaying> {
    let mut selected = None;
    for track in tracks {
        if track.playing {
            return Some(track);
        }
        if selected.is_none() {
            selected = Some(track);
        }
    }
    selected
}

fn now_playing() -> (Option<NowPlaying>, Option<String>) {
    let mut tracks = Vec::new();
    let mut error = None;
    for player in [Player::Spotify, Player::Music, Player::ITunes] {
        if !player.installed() {
            continue;
        }
        match run(&query(player)) {
            Ok(output) => {
                if let Some(mut track) = parse(&output) {
                    track.player = player;
                    tracks.push(track);
                }
            }
            Err(message) => {
                if error.is_none() {
                    error = Some(player_error(player, &message));
                }
            }
        }
    }
    (choose_player(tracks), error)
}

fn run(script: &str) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Ok(String::new());
    }
    let mut child = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("The player did not respond in time.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Parses `state \t name \t artist \t position-seconds \t duration-ms \t artwork-url`.
fn parse(line: &str) -> Option<NowPlaying> {
    let mut parts = line.split('\t');
    let state = parts.next()?;
    if !matches!(state, "playing" | "paused") {
        return None;
    }
    let title = parts.next()?.to_owned();
    let artist = parts.next()?.to_owned();
    let number =
        |s: Option<&str>| s.and_then(|s| s.trim().replace(',', ".").parse::<f32>().ok()).filter(|n| n.is_finite());
    let position = number(parts.next()).unwrap_or(0.);
    let duration_ms = number(parts.next()).unwrap_or(0.);
    let progress = if duration_ms > 0. { (position * 1000. / duration_ms).clamp(0., 1.) } else { 0. };
    let artwork_url = parts.next().map(str::trim).filter(|u| !u.is_empty() && *u != "missing value").map(String::from);
    Some(NowPlaying { player: Player::Spotify, title, artist, playing: state == "playing", progress, artwork_url })
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
        .user_agent("Roughcut")
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
    fn selects_playing_music_over_paused_spotify_and_uses_music_seconds() {
        use super::{Player, choose_player, player_error, query};
        let spotify = parse("paused\tOne\tArtist\t20\t200000\t").unwrap();
        let mut music = parse("playing\tTwo\tArtist\t30\t120000\t").unwrap();
        music.player = Player::Music;
        assert_eq!(choose_player([spotify, music.clone()]), Some(music));
        assert!(query(Player::Music).contains("(duration of t) * 1000"));
        assert!(query(Player::ITunes).contains("application \"iTunes\""));
        assert!(player_error(Player::Spotify, "Not authorized (-1743)").contains("Automation"));
        assert_eq!(parse("stopped\tTrack\tArtist\t0\t1\t"), None);
        assert_eq!(parse("playing\tTrack\tArtist\tNaN\tInfinity\t").unwrap().progress, 0.);
    }

    #[test]
    #[ignore = "reads local media apps through macOS automation"]
    fn reads_running_players() {
        let (now, error) = super::now_playing();
        assert!(error.is_none(), "{error:?}");
        if let Some(track) = now {
            assert!(track.progress.is_finite());
            println!("{} status and track metadata received (playing: {}).", track.player.name(), track.playing);
        } else {
            println!("No running player has a current track.");
        }
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
