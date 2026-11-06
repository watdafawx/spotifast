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
/// 2: picks carry the offset measured from the video's captions.
const CACHE_VERSION: u32 = 2;

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
    /// The song's timed lyric lines (start, words), to line the video's
    /// captions up with; empty when the lyrics carry no timing.
    pub lyrics: Vec<(u32, String)>,
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
    let mut found = pick(&String::from_utf8_lossy(&output.stdout), request);
    if let Some(info) = &mut found {
        sync_from_captions(cache_dir, info, request);
    }
    write_cache(&path, &found);
    Ok(found)
}

/// Lines the video up with the song from its captions: the music video's
/// own subtitles or YouTube's automatic ones, matched against the song's
/// timed lyrics. Leaves the video on the song's clock when they do not
/// agree.
fn sync_from_captions(cache_dir: &Path, info: &mut VideoInfo, request: &Request) {
    let Some(&(first, _)) = request
        .lyrics
        .iter()
        .find(|(_, words)| !words.trim().is_empty())
    else {
        return;
    };
    let folder = cache_dir.join(format!("captions-{}", info.youtube_id));
    let _ = std::fs::create_dir_all(&folder);
    let mut command = std::process::Command::new(yt_dlp());
    command
        .args([
            "--no-update",
            "--no-warnings",
            "--skip-download",
            "--write-subs",
            "--write-auto-subs",
            "--sub-langs",
            ".*-orig,en,ko,ja,zh-Hans,zh-Hant,es,pt,fr,de",
            "--sub-format",
            "vtt",
            "-o",
        ])
        .arg(folder.join("%(id)s"))
        .arg(format!(
            "https://www.youtube.com/watch?v={}",
            info.youtube_id
        ))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = command.status();
    let mut cues = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&folder) {
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                cues.extend(parse_vtt(&text));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&folder);
    match estimate_offset(&cues, &request.lyrics) {
        Some(offset) => {
            log::info!(
                "lyrics video for {}: captions put the video {offset:+.1}s from the song",
                request.uri
            );
            info.caption_start = Some(f64::from(first) / 1000.0 + offset);
            info.automatic = false;
        }
        None => log::info!(
            "lyrics video for {}: no captions to sync by ({} cues)",
            request.uri,
            cues.len()
        ),
    }
}

/// The cues of a WebVTT file: start in seconds and words, with timing
/// tags, entities, notes and music symbols taken out.
pub fn parse_vtt(text: &str) -> Vec<(f64, String)> {
    fn seconds(stamp: &str) -> Option<f64> {
        let mut total = 0.0;
        for part in stamp.trim().split(':') {
            total = total * 60.0 + part.parse::<f64>().ok()?;
        }
        Some(total)
    }
    let mut cues = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some((start, _)) = line.split_once("-->") else {
            continue;
        };
        let Some(start) = seconds(start) else {
            continue;
        };
        let mut words = String::new();
        for line in lines.by_ref() {
            if line.trim().is_empty() {
                break;
            }
            let mut plain = String::new();
            let mut tag = false;
            for c in line.chars() {
                match c {
                    '<' => tag = true,
                    '>' if tag => tag = false,
                    _ if !tag => plain.push(c),
                    _ => {}
                }
            }
            let plain = plain
                .replace("&gt;", "")
                .replace("&lt;", "")
                .replace("&amp;", "&")
                .replace("[music]", "")
                .replace("[Music]", "")
                .replace('♪', "");
            if !plain.trim().is_empty() {
                words.push(' ');
                words.push_str(plain.trim());
            }
        }
        if !words.trim().is_empty() {
            cues.push((start, words.trim().to_string()));
        }
    }
    cues
}

/// Letters and digits only, lower case.
fn squash(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// How alike two lines are, 0 to 1, by the pairs of letters they share.
fn likeness(a: &[char], b: &[char]) -> f64 {
    if a.len() < 2 || b.len() < 2 {
        return if a == b && !a.is_empty() { 1.0 } else { 0.0 };
    }
    let pairs =
        |text: &[char]| -> Vec<(char, char)> { text.windows(2).map(|w| (w[0], w[1])).collect() };
    let (left, mut right) = (pairs(a), pairs(b));
    let total = left.len() + right.len();
    let mut shared = 0;
    for pair in left {
        if let Some(at) = right.iter().position(|other| *other == pair) {
            right.swap_remove(at);
            shared += 1;
        }
    }
    2.0 * shared as f64 / total as f64
}

/// The video's offset from the song in seconds, from caption cues and the
/// song's timed lines. Every lyric line that a caption matches votes for
/// the gap between them; repeated choruses vote for every repeat, so the
/// gap most lines agree on wins. `None` unless three different lines
/// agree within a second.
pub fn estimate_offset(cues: &[(f64, String)], lyrics: &[(u32, String)]) -> Option<f64> {
    let cues: Vec<(f64, Vec<char>)> = cues
        .iter()
        .map(|(at, words)| (*at, squash(words)))
        .collect();
    let mut votes: Vec<(usize, f64)> = Vec::new();
    for (line, (at, words)) in lyrics.iter().enumerate() {
        let words = squash(words);
        if words.len() < 4 {
            continue;
        }
        for (start, cue) in &cues {
            // A cue may hold this line and the next; compare like lengths.
            let near = cue.len() <= words.len() * 2 + 4;
            if near && likeness(&words, cue) >= 0.6 {
                votes.push((line, start - f64::from(*at) / 1000.0));
            }
        }
    }
    let mut best: Option<(usize, f64)> = None;
    for (_, gap) in &votes {
        let agreeing: Vec<&(usize, f64)> = votes
            .iter()
            .filter(|(_, other)| (other - gap).abs() <= 1.0)
            .collect();
        let mut lines: Vec<usize> = agreeing.iter().map(|(line, _)| *line).collect();
        lines.sort_unstable();
        lines.dedup();
        if best.is_none_or(|(held, _)| lines.len() > held) {
            let mut gaps: Vec<f64> = agreeing.iter().map(|(_, gap)| *gap).collect();
            gaps.sort_by(f64::total_cmp);
            best = Some((lines.len(), gaps[gaps.len() / 2]));
        }
    }
    best.filter(|(lines, _)| *lines >= 3).map(|(_, gap)| gap)
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
        // The captions are lined up with the lyrics, so wait for them.
        let lyrics = match &app.lyrics {
            _ if app.lyrics_uri.as_deref() != Some(now.uri.as_str()) => return,
            crate::model::Loadable::NotLoaded | crate::model::Loadable::Loading => return,
            crate::model::Loadable::Loaded(Some(lyrics)) if lyrics.synced => lyrics
                .lines
                .iter()
                .filter_map(|line| Some((line.at_ms?, line.text.clone())))
                .collect(),
            _ => Vec::new(),
        };
        let state = &mut app.lyrics_video;
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
                    lyrics,
                },
            )));
        return;
    }
    let state = &mut app.lyrics_video;
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
            lyrics: Vec::new(),
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
    fn captions_line_the_video_up_with_the_lyrics() {
        let vtt = "WEBVTT\nKind: captions\n\n00:00:13.665 --> 00:00:15.458\n♪ I’m a fire starter ♪\n\n\
                   00:00:15.458 --> 00:00:17.208\n♪ Work a lil harder ♪\n\n\
                   00:00:17.208 --> 00:00:18.623\n<c>Got</c> me fired up\n\n\
                   00:00:20.916 --> 00:00:22.750\nImma burn it down like\n\n\
                   00:01:13.665 --> 00:01:15.458\nI'm a fire starter\n";
        let cues = parse_vtt(vtt);
        assert_eq!(cues[0].1, "I’m a fire starter");
        assert_eq!(cues[2].1, "Got me fired up");
        assert!((cues[4].0 - 73.665).abs() < 0.001);
        // The song sings the same lines 5 s earlier than the video.
        let lyrics = vec![
            (8_600, "I'm a fire starter".to_string()),
            (10_400, "Work a lil harder".to_string()),
            (12_200, "Got me fired up".to_string()),
            (15_900, "Imma burn it down like".to_string()),
        ];
        let offset = estimate_offset(&cues, &lyrics).unwrap();
        assert!((offset - 5.0).abs() < 0.2, "{offset}");
        // Two matching lines are not enough to trust.
        assert_eq!(estimate_offset(&cues, &lyrics[..2]), None);
        assert_eq!(estimate_offset(&[], &lyrics), None);
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
