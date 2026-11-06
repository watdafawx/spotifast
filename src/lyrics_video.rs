//! Fork: music videos behind full-screen lyrics, the native replacement
//! for ivLyrics' video background.
//!
//! The video is found with a YouTube search through yt-dlp ("artist title
//! official music video"), on the backend, and the pick is cached for a
//! week (a day when nothing fits). mpv streams it through yt-dlp and draws
//! it into the window ([`mpv`], Windows only); [`target`] keeps it on the
//! song's clock. ivLyrics' community service is not used: its operator
//! answers other apps with a warning video.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
pub mod mpv;

/// Search results change; a week keeps lookups rare and picks fresh.
const FOUND_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A song with no fitting video is searched again after a day.
const MISSING_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_VERSION: u32 = 1;

/// The video for one recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoInfo {
    pub youtube_id: String,
    /// Seconds into the video where the first lyric is sung.
    pub caption_start: Option<f64>,
    /// Matched by the service rather than picked by a person; a caption
    /// start of 0 then means "unknown".
    pub automatic: bool,
    /// Stretches of the video with no song in them, skipped over.
    pub skip: Vec<Segment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
}

/// Words in a result's title that mean it is not the music video.
const NOT_A_VIDEO: &[&str] = &[
    "lyric",
    "lyrics",
    "audio",
    "visualizer",
    "visualiser",
    "karaoke",
    "instrumental",
    "cover",
    "reaction",
    "remix",
    "sped up",
    "slowed",
    "8d",
    "live",
    "dance practice",
    "teaser",
    "shorts",
];

/// Picks the music video from yt-dlp's search results, one
/// `id<TAB>duration<TAB>title` line each: the first whose title names the
/// song, is not a lyric or audio upload, and is about as long as the song
/// (videos may add an intro or an outro, so up to two minutes longer).
pub fn pick(results: &str, request: &Request) -> Option<VideoInfo> {
    let song = request.title.to_lowercase();
    let song = crate::lyrics::clean_title(&song);
    let length = f64::from(request.duration_ms) / 1000.0;
    let mut fallback = None;
    for line in results.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(id), Some(duration), Some(title)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let id = id.trim();
        // Eleven characters of the YouTube alphabet, nothing that could
        // become a different URL or an mpv option.
        if id.len() != 11
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            continue;
        }
        let title = title.to_lowercase();
        if !song.is_empty() && !title.contains(song.as_str()) {
            continue;
        }
        if NOT_A_VIDEO
            .iter()
            .any(|word| title.contains(word) && !song.contains(word))
        {
            continue;
        }
        let fits = duration.trim().parse::<f64>().is_ok_and(|duration| {
            length <= 0.0 || (duration >= length - 15.0 && duration <= length + 120.0)
        });
        if !fits {
            continue;
        }
        let info = VideoInfo {
            youtube_id: id.to_string(),
            caption_start: None,
            automatic: true,
            skip: Vec::new(),
        };
        if title.contains("official") || title.contains(" mv") || title.contains("m/v") {
            return Some(info);
        }
        fallback.get_or_insert(info);
    }
    fallback
}

/// Where the video should be for the song's position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// Before the video's own beginning: hold its first frame.
    Hold,
    /// Seconds into the video.
    At(f64),
}

/// The video time for `position_ms` into the song, given when the song's
/// first lyric starts. The video is shifted so its first sung line meets
/// the song's, and skipped stretches are jumped over.
pub fn target(position_ms: u32, lyrics_start_ms: Option<u32>, info: &VideoInfo) -> Target {
    let offset = match (info.caption_start, lyrics_start_ms) {
        // An automatic match's 0 is "unknown": play from the song's start.
        (Some(caption), _) if info.automatic && caption.abs() < 0.001 => 0.0,
        (Some(caption), Some(lyrics)) => caption - f64::from(lyrics) / 1000.0,
        _ => 0.0,
    };
    let base = f64::from(position_ms) / 1000.0 + offset;
    if base < 0.0 {
        return Target::Hold;
    }
    let anchor = info.caption_start.unwrap_or(0.0).max(0.0);
    let mut mapped = base;
    for segment in &info.skip {
        if segment.end <= anchor {
            continue;
        }
        let start = anchor.max(segment.start);
        if mapped + 0.001 >= start {
            mapped += segment.end - start;
        }
    }
    Target::At(mapped.max(0.0))
}

// ---- lookup ----------------------------------------------------------------

/// What the backend needs to ask for a song's video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub uri: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub duration_ms: u32,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    found: Option<VideoInfo>,
}

pub fn cache_dir(lyrics_cache: &Path) -> PathBuf {
    lyrics_cache.join("videos")
}

/// The song's music video, from the cache while it is fresh, else from a
/// YouTube search through yt-dlp. Blocking: run it off the UI thread.
pub fn find(cache_dir: &Path, request: &Request) -> Result<Option<VideoInfo>, String> {
    let Some(id) = request
        .uri
        .strip_prefix("spotify:track:")
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
    else {
        return Ok(None);
    };
    let path = cache_dir.join(format!("{id}.json"));
    if let Some(cached) = read_cache(&path) {
        log::debug!("lyrics video for {}: from the cache", request.uri);
        return Ok(cached);
    }
    let artist = request.artists.first().cloned().unwrap_or_default();
    let query = format!(
        "ytsearch8:{} {} official music video",
        crate::lyrics::clean_artist(&artist),
        crate::lyrics::clean_title(&request.title)
    );
    log::info!("lyrics video for {}: searching YouTube", request.uri);
    let mut command = std::process::Command::new(yt_dlp());
    command
        .args([
            "--no-update",
            "--no-warnings",
            "--flat-playlist",
            "--print",
            "%(id)s\t%(duration)s\t%(title)s",
        ])
        .arg(&query)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // No console window flashing up for the search.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run yt-dlp: {error}"))?;
    if !output.status.success() {
        return Err(format!("yt-dlp failed ({})", output.status));
    }
    let found = pick(&String::from_utf8_lossy(&output.stdout), request);
    write_cache(&path, &found);
    Ok(found)
}

/// yt-dlp beside the app, else on the search path.
pub fn yt_dlp() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            Some(exe.parent()?.join(if cfg!(windows) {
                "yt-dlp.exe"
            } else {
                "yt-dlp"
            }))
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("yt-dlp"))
}

fn read_cache(path: &Path) -> Option<Option<VideoInfo>> {
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    let cached: Cached = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if cached.version != CACHE_VERSION {
        return None;
    }
    let lifetime = if cached.found.is_some() {
        FOUND_LIFETIME
    } else {
        MISSING_LIFETIME
    };
    (age < lifetime).then_some(cached.found)
}

fn write_cache(path: &Path, found: &Option<VideoInfo>) {
    let Ok(bytes) = serde_json::to_vec(&Cached {
        version: CACHE_VERSION,
        found: found.clone(),
    }) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = fastframe_file::write(path, bytes) {
        log::debug!("lyrics video cache write failed: {error}");
    }
}

// ---- app state -------------------------------------------------------------

/// What the app holds for the video behind the lyrics.
#[derive(Default)]
pub struct State {
    /// The track the lookup below is for.
    pub uri: Option<String>,
    /// `None` while asking; `Some(None)` when the song has no video.
    pub info: Option<Option<VideoInfo>>,
    #[cfg(windows)]
    pub player: Option<std::sync::Arc<mpv::Player>>,
    /// When the video was last on screen, so mpv is let go of a while
    /// after full screen closes.
    pub last_shown: Option<std::time::Instant>,
    /// When the player was last brought in line with the song.
    pub last_sync: Option<std::time::Instant>,
    /// When the player last seeked: a stream takes a moment to land, so
    /// it is not chased again at once.
    pub last_seek: Option<std::time::Instant>,
    /// mpv could not start; not tried again this run.
    pub failed: bool,
}

/// Whether video backgrounds can work here: Windows with libmpv.
pub fn available() -> bool {
    #[cfg(windows)]
    {
        mpv::available()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// How often the video is brought in line with the song.
const SYNC_EVERY: Duration = Duration::from_millis(250);
/// Drift this large is corrected with a seek.
const SEEK_THRESHOLD: f64 = 0.5;
/// After a seek, the stream gets this long to land before another.
const SEEK_SETTLE: Duration = Duration::from_secs(3);
/// The video stops this long after full screen closes; the player stays,
/// idle, for the next time.
const LINGER: Duration = Duration::from_secs(30);

/// Once a frame: finds the playing song's video while full-screen lyrics
/// are open, and keeps mpv playing it in time with the song.
pub fn sync(app: &mut crate::app::App, ctx: &egui::Context) {
    #[cfg(windows)]
    sync_windows(app, ctx);
    #[cfg(not(windows))]
    let _ = (app, ctx);
}

#[cfg(windows)]
fn sync_windows(app: &mut crate::app::App, ctx: &egui::Context) {
    use std::time::Instant;
    let showing = app.lyrics_fullscreen.is_some() && app.settings.lyrics_video && available();
    let now_playing = app.now_playing().filter(|now| !now.is_episode);
    let state = &mut app.lyrics_video;
    let (Some(now), true) = (now_playing, showing) else {
        if let Some(player) = &state.player
            && player.loaded().is_some()
            && state.last_shown.is_none_or(|at| at.elapsed() > LINGER)
        {
            player.stop();
        }
        return;
    };
    state.last_shown = Some(Instant::now());
    if state.uri.as_deref() != Some(now.uri.as_str()) {
        state.uri = Some(now.uri.clone());
        state.info = None;
        if let Some(player) = &state.player {
            player.stop();
        }
        app.backend
            .send(crate::backend::Command::FindLyricsVideo(Box::new(
                Request {
                    uri: now.uri.clone(),
                    title: now.title.clone(),
                    artists: now
                        .artists
                        .iter()
                        .map(|artist| artist.name.clone())
                        .collect(),
                    album: now.album_name.clone(),
                    duration_ms: now.duration_ms,
                },
            )));
        return;
    }
    let Some(Some(info)) = state.info.clone() else {
        return;
    };
    if state.player.is_none() && !state.failed {
        match mpv::Player::new(ctx) {
            Some(player) => state.player = Some(std::sync::Arc::new(player)),
            None => {
                log::warn!("lyrics video: mpv could not start");
                state.failed = true;
            }
        }
    }
    let Some(player) = state.player.clone() else {
        return;
    };
    player.poll();
    if state.last_sync.is_some_and(|at| at.elapsed() < SYNC_EVERY) {
        ctx.request_repaint_after(SYNC_EVERY);
        return;
    }
    state.last_sync = Some(Instant::now());
    ctx.request_repaint_after(SYNC_EVERY);
    let lyrics_start = match &app.lyrics {
        crate::model::Loadable::Loaded(Some(lyrics))
            if lyrics.synced && app.lyrics_uri.as_deref() == Some(now.uri.as_str()) =>
        {
            lyrics
                .lines
                .iter()
                .find(|line| !line.text.trim().is_empty())
                .and_then(|line| line.at_ms)
        }
        _ => None,
    };
    let (time, duration, _) = player.position();
    let (wanted, hold) = match target(now.position_ms, lyrics_start, &info) {
        Target::Hold => (0.0, true),
        // A video shorter than the song starts over, as ivLyrics does.
        Target::At(at) => (
            match duration.filter(|length| *length > 1.0) {
                Some(length) => at % length,
                None => at,
            },
            false,
        ),
    };
    let paused = hold || !now.playing;
    if !player.can_draw() {
        // The next frame's paint callback sets mpv's drawing up.
        ctx.request_repaint();
        return;
    }
    if player.loaded().as_deref() != Some(info.youtube_id.as_str()) {
        player.load(&info.youtube_id, wanted, paused);
        state.last_seek = Some(Instant::now());
        return;
    }
    player.set_paused(paused);
    let settled = state.last_seek.is_none_or(|at| at.elapsed() > SEEK_SETTLE);
    if settled && time.is_none_or(|time| (time - wanted).abs() > SEEK_THRESHOLD) {
        player.seek(wanted);
        state.last_seek = Some(Instant::now());
    }
}

/// Draws the video over `rect` when a frame of the playing song's video is
/// ready. `false` leaves the cover to the caller.
pub fn paint(app: &crate::app::App, painter: &egui::Painter, rect: egui::Rect) -> bool {
    #[cfg(windows)]
    {
        let state = &app.lyrics_video;
        let Some(Some(info)) = &state.info else {
            return false;
        };
        let Some(player) = state.player.clone() else {
            return false;
        };
        if !player.can_draw() {
            // Draws nothing; gives mpv its OpenGL context to load into.
            painter.add(egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(move |_, _| {
                    player.prepare()
                })),
            });
            return false;
        }
        let playing = app.now_playing().map(|now| now.uri);
        if playing != state.uri
            || player.loaded().as_deref() != Some(info.youtube_id.as_str())
            || !player.position().2
        {
            return false;
        }
        painter.add(egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                move |info, painter| player.paint(painter.gl(), &info),
            )),
        });
        true
    }
    #[cfg(not(windows))]
    {
        let _ = (app, painter, rect);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(caption: Option<f64>, automatic: bool, skip: Vec<Segment>) -> VideoInfo {
        VideoInfo {
            youtube_id: "dQw4w9WgXcQ".into(),
            caption_start: caption,
            automatic,
            skip,
        }
    }

    #[test]
    fn the_search_picks_the_music_video() {
        let request = Request {
            uri: "spotify:track:abc".into(),
            title: "FIRESTARTER".into(),
            artists: vec!["K/DA".into()],
            album: String::new(),
            duration_ms: 187_000,
        };
        let results = "aaaaaaaaaaa\t190\tK/DA - FIRESTARTER (Lyrics)\n\
                       bbbbbbbbbbb\t400\tFIRESTARTER 1 hour loop\n\
                       x --script=y\t190\tFIRESTARTER\n\
                       ccccccccccc\t200\tK/DA FIRESTARTER teaser\n\
                       ddddddddddd\t215\tFIRESTARTER - K/DA\n\
                       eeeeeeeeeee\t230\tK/DA - FIRESTARTER (Official Music Video)\n";
        assert_eq!(pick(results, &request).unwrap().youtube_id, "eeeeeeeeeee");
        // Without an official upload the first fitting result serves.
        let unofficial = "ddddddddddd\t215\tFIRESTARTER - K/DA\n";
        assert_eq!(
            pick(unofficial, &request).unwrap().youtube_id,
            "ddddddddddd"
        );
        assert_eq!(pick("", &request), None);
    }

    #[test]
    fn the_video_meets_the_song_at_the_first_lyric() {
        // The first lyric is sung 20 s into the video and 5 s into the song.
        let picked = info(Some(20.0), false, vec![]);
        assert_eq!(target(5_000, Some(5_000), &picked), Target::At(20.0));
        assert_eq!(target(0, Some(5_000), &picked), Target::At(15.0));
        // A video whose singing starts earlier than the song's holds still.
        let early = info(Some(2.0), false, vec![]);
        assert_eq!(target(1_000, Some(5_000), &early), Target::Hold);
        assert_eq!(target(4_000, Some(5_000), &early), Target::At(1.0));
        // An automatic match's 0 means unknown: the song's own clock.
        let automatic = info(Some(0.0), true, vec![]);
        assert_eq!(target(7_000, Some(5_000), &automatic), Target::At(7.0));
        // Without timed lyrics the video runs with the song.
        assert_eq!(target(7_000, None, &picked), Target::At(7.0));
    }

    #[test]
    fn skipped_stretches_are_jumped_over() {
        let skipping = info(
            Some(10.0),
            false,
            vec![Segment {
                start: 30.0,
                end: 40.0,
            }],
        );
        // Song 15 s with lyrics at 0 s: video 25 s, before the skip.
        assert_eq!(target(15_000, Some(0), &skipping), Target::At(25.0));
        // Song 25 s: video 35 s falls in the skip and lands 10 s later.
        assert_eq!(target(25_000, Some(0), &skipping), Target::At(45.0));
    }

    #[test]
    fn answers_are_cached_a_week_and_misses_a_day() {
        let dir = std::env::temp_dir().join(format!("spotifast-videos-{}", std::process::id()));
        let path = dir.join("USRC17607839.json");
        assert_eq!(read_cache(&path), None);
        let found = Some(info(Some(1.0), false, vec![]));
        write_cache(&path, &found);
        assert_eq!(read_cache(&path), Some(found));
        write_cache(&path, &None);
        assert_eq!(read_cache(&path), Some(None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
