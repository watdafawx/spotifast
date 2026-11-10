//! Fork: the lyrics page's settings and the per-song lyrics offset.
//!
//! The page (src/ui/fork/lyrics_page.rs) takes over the main area, as ivLyrics'
//! page does in Spotify: the song's music video or cover behind, blurred
//! and dimmed as the listener likes, the lyrics over it, and a rail of
//! controls. A song whose lyrics run early or late gets an offset of its
//! own, which every lyrics view follows.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Offsets kept at most; past this the oldest-named go first.
const MAX_OFFSETS: usize = 2000;
/// The furthest a song's lyrics may be moved, either way.
pub const MAX_OFFSET_MS: i32 = 10_000;

/// Stored in `settings.json` under `lyrics_page`; older files without it
/// get these defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageSettings {
    /// Blur over the video, in video pixels; 0 is sharp.
    pub blur: f32,
    /// How much the backdrop is darkened, 0 to 0.9.
    pub dim: f32,
    /// Lyrics size, as a share of the usual.
    pub text_scale: f32,
    /// Lines centred rather than at the left.
    pub centred: bool,
    /// How the sung line stands out.
    pub style: LineStyle,
    /// The sung line lights up word by word.
    pub word_highlight: bool,
    /// Only the sung line shows, sliding in and out.
    pub single_line: bool,
    /// A translated line shows its translation as the line, the original
    /// small above.
    pub translation_first: bool,
    /// The view follows the song; scrolling by hand pauses it briefly.
    pub follow: bool,
    /// The control rail shows its labels.
    pub rail_labels: bool,
    /// How a line arrives, moves while sung, and leaves.
    pub enter: Enter,
    pub motion: Motion,
    pub leave: Leave,
    /// A soft glow around the sung line.
    pub glow: bool,
    /// A dark outline around the lines.
    pub outline: bool,
    /// Dots through instrumental breaks.
    pub break_dots: bool,
    /// How bright the lines before and after the sung one are, 0.1 to 1.
    pub others: f32,
    /// Slow colour blobs behind the lyrics when there is no video.
    pub blobs: bool,
    /// Per song (by track URI), how many milliseconds earlier the lyrics
    /// run: positive when they lag the singing.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub offsets: BTreeMap<String, i32>,
}

impl Default for PageSettings {
    fn default() -> Self {
        Self {
            blur: 12.0,
            dim: 0.55,
            text_scale: 1.0,
            centred: true,
            style: LineStyle::Classic,
            word_highlight: false,
            single_line: false,
            translation_first: true,
            follow: true,
            rail_labels: false,
            enter: Enter::Fade,
            motion: Motion::None,
            leave: Leave::Fade,
            glow: false,
            outline: false,
            break_dots: true,
            others: 0.68,
            blobs: false,
            offsets: BTreeMap::new(),
        }
    }
}

/// How the sung line stands out from the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineStyle {
    /// Bright sung line, the others quiet.
    #[default]
    Classic,
    /// The sung line in the accent colour.
    Accent,
    /// Lines fade the further they are from the sung one.
    Fade,
}

impl LineStyle {
    pub const ALL: [Self; 3] = [Self::Classic, Self::Accent, Self::Fade];
}

/// How the sung line arrives (in the single-line view).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enter {
    #[default]
    Fade,
    Rise,
    Typewriter,
    Zoom,
}

impl Enter {
    pub const ALL: [Self; 4] = [Self::Fade, Self::Rise, Self::Typewriter, Self::Zoom];
}

/// What the sung line does while it is sung.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    #[default]
    None,
    Float,
    Pulse,
}

impl Motion {
    pub const ALL: [Self; 3] = [Self::None, Self::Float, Self::Pulse];
}

/// How a finished line goes (in the single-line view).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Leave {
    #[default]
    Fade,
    DriftUp,
    Zoom,
}

impl Leave {
    pub const ALL: [Self; 3] = [Self::Fade, Self::DriftUp, Self::Zoom];
}

/// How a lyrics view draws its lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub scale: f32,
    pub centred: bool,
    pub style: LineStyle,
    pub word_highlight: bool,
    pub single_line: bool,
    pub translation_first: bool,
    /// The view follows the song.
    pub follow: bool,
    pub enter: Enter,
    pub motion: Motion,
    pub leave: Leave,
    pub glow: bool,
    pub outline: bool,
    pub break_dots: bool,
    pub others: f32,
    /// Where the sung line sits, as a share of the view from the top.
    pub sung_at: f32,
}

impl Look {
    /// Full-screen lyrics as they have always been.
    pub const FULLSCREEN: Self = Self {
        scale: 1.0,
        centred: false,
        style: LineStyle::Classic,
        word_highlight: false,
        single_line: false,
        translation_first: false,
        follow: true,
        enter: Enter::Fade,
        motion: Motion::None,
        leave: Leave::Fade,
        glow: false,
        outline: false,
        break_dots: false,
        // Upstream's quiet lines over the cover.
        others: 0.42,
        sung_at: 0.2,
    };
}

/// How far through line `index` the singing is, 0 to 1: from its start to
/// the next line's (or five seconds on, for the last).
pub fn line_progress(starts: &[Option<u32>], index: usize, position_ms: u32) -> f32 {
    let Some(Some(start)) = starts.get(index).copied() else {
        return 0.0;
    };
    let end = starts
        .iter()
        .skip(index + 1)
        .find_map(|at| *at)
        .filter(|end| *end > start)
        .unwrap_or(start + 5_000);
    ((position_ms.saturating_sub(start)) as f32 / (end - start) as f32).clamp(0.0, 1.0)
}

/// Where each word ends, as a share of the line, by its letters: lyrics
/// carry no word timing, so longer words are taken to last longer.
pub fn word_ends<S: AsRef<str>>(words: &[S]) -> Vec<f32> {
    let weight = |word: &str| word.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f32;
    let total: f32 = words.iter().map(|word| weight(word.as_ref())).sum();
    let mut done = 0.0;
    words
        .iter()
        .map(|word| {
            done += weight(word.as_ref());
            done / total.max(1.0)
        })
        .collect()
}

impl PageSettings {
    pub fn look(&self) -> Look {
        Look {
            scale: self.text_scale.clamp(0.6, 1.8),
            centred: self.centred,
            style: self.style,
            word_highlight: self.word_highlight,
            single_line: self.single_line,
            translation_first: self.translation_first,
            follow: self.follow,
            enter: self.enter,
            motion: self.motion,
            leave: self.leave,
            glow: self.glow,
            outline: self.outline,
            break_dots: self.break_dots,
            others: self.others.clamp(0.1, 1.0),
            // The page keeps the sung line in the middle, as ivLyrics does.
            sung_at: 0.45,
        }
    }

    pub fn offset(&self, uri: &str) -> i32 {
        self.offsets.get(uri).copied().unwrap_or(0)
    }

    /// Moves a song's lyrics by `delta` ms, or back to none with 0.
    pub fn nudge(&mut self, uri: &str, delta: i32) {
        let offset = if delta == 0 {
            0
        } else {
            (self.offset(uri) + delta).clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS)
        };
        if offset == 0 {
            self.offsets.remove(uri);
            return;
        }
        self.offsets.insert(uri.to_string(), offset);
        while self.offsets.len() > MAX_OFFSETS {
            let Some(first) = self.offsets.keys().next().cloned() else {
                break;
            };
            self.offsets.remove(&first);
        }
    }

    /// Where the lyrics are for a song at `position_ms`, its offset applied.
    pub fn position(&self, uri: &str, position_ms: u32) -> u32 {
        let moved = i64::from(position_ms) + i64::from(self.offset(uri));
        u32::try_from(moved.max(0)).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_songs_offset_moves_its_lyrics_and_only_its_own() {
        let mut page = PageSettings::default();
        page.nudge("spotify:track:a", 100);
        page.nudge("spotify:track:a", 50);
        assert_eq!(page.offset("spotify:track:a"), 150);
        assert_eq!(page.position("spotify:track:a", 1_000), 1_150);
        assert_eq!(page.position("spotify:track:b", 1_000), 1_000);
        page.nudge("spotify:track:a", -300);
        assert_eq!(
            page.position("spotify:track:a", 100),
            0,
            "never before the start"
        );
        page.nudge("spotify:track:a", 0);
        assert!(page.offsets.is_empty(), "back to none forgets the song");
        page.nudge("spotify:track:a", 60_000);
        assert_eq!(page.offset("spotify:track:a"), MAX_OFFSET_MS);
    }

    #[test]
    fn words_light_up_through_the_line() {
        let starts = [Some(1_000), None, Some(3_000)];
        assert_eq!(line_progress(&starts, 0, 500), 0.0);
        assert_eq!(line_progress(&starts, 0, 2_000), 0.5);
        assert_eq!(line_progress(&starts, 0, 9_000), 1.0);
        assert_eq!(
            line_progress(&starts, 2, 5_500),
            0.5,
            "the last line runs five seconds"
        );
        let ends = word_ends(&["I", "love", "you"]);
        assert_eq!(ends, vec![0.125, 0.625, 1.0]);
    }

    #[test]
    fn older_settings_files_get_the_defaults() {
        let page: PageSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(page, PageSettings::default());
        let json = serde_json::to_string(&PageSettings::default()).unwrap();
        assert!(!json.contains("offsets"));
    }
}
