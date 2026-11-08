//! Now-playing for the Spotify desktop app, controlled through AppleScript.
//! Quill talks only to the app already running on this Mac — no network, no
//! account — and checks that Spotify is running before addressing it, so it
//! never launches Spotify on its own.

use std::process::Command;
use std::time::Duration;

use gpui_kit::*;

const POLL_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// 0.0..=1.0 through the track.
    pub progress: f32,
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
        Media { now: None, _poll: poll }
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
        return s & tab & (name of t) & tab & (artist of t) & tab & (player position as string) & tab & ((duration of t) as string)
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

/// Parses `state \t name \t artist \t position-seconds \t duration-ms`.
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
    Some(NowPlaying { title, artist, playing: state == "playing", progress })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_osascript_output() {
        let now = parse("playing\tGymnopédie No. 1\tErik Satie\t76,5\t204000").unwrap();
        assert_eq!(now.title, "Gymnopédie No. 1");
        assert_eq!(now.artist, "Erik Satie");
        assert!(now.playing);
        assert!((now.progress - 0.375).abs() < 0.001);
        assert!(!parse("paused\tA\tB\t0\t0").unwrap().playing);
        assert_eq!(parse(""), None);
    }
}
