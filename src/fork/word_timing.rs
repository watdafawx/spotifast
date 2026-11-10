//! Fork: real word timing for the lyrics page's word-by-word light.
//!
//! Spotify and LRCLIB time whole lines. Unison (unison.boidu.dev, the
//! open, crowdsourced lyrics API behind Better Lyrics) often has the same
//! song timed word by word, as Apple Music's TTML. It is asked by ISRC,
//! then by title and artist, and the answer is cached (30 days, 3 when
//! there is none). Its lines are matched to the lyrics on screen by their
//! words and start, so what is shown never changes; a matched line gets
//! each letter's time, and the light follows the singing exactly. Lines
//! without a match keep the estimate from their length.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const API: &str = "https://unison.boidu.dev/lyrics";
const FOUND_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MISSING_LIFETIME: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const CACHE_VERSION: u32 = 1;
/// A timed line this far from the shown line's start is another line.
const MAX_START_GAP_MS: u32 = 3_000;

/// Each character of a shown line with when it is sung.
pub type CharTimes = Arc<Vec<(u32, u32)>>;

/// One word and when it is sung.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

/// One timed line: when it starts, and its words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedLine {
    pub start_ms: u32,
    pub words: Vec<Word>,
}

/// What the backend needs to look a song up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
}

/// `m:ss.mmm`, `h:mm:ss.mmm`, `ss.mmm` or `12.3s` in milliseconds.
fn parse_time(text: &str) -> Option<u32> {
    let text = text.trim().trim_end_matches('s');
    let mut seconds = 0.0f64;
    for part in text.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    (seconds.is_finite() && seconds >= 0.0).then(|| (seconds * 1000.0).round() as u32)
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let rest = &tag[at..];
    Some(&rest[..rest.find('"')?])
}

fn unescape(text: &str) -> String {
    text.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The word-timed lines of a TTML document. Background vocals
/// (`ttm:role="x-bg"`) are left out: the shown lyrics carry only the lead.
pub fn parse_ttml(ttml: &str) -> Vec<TimedLine> {
    let mut lines = Vec::new();
    let mut rest = ttml;
    while let Some(open) = rest.find("<p ") {
        rest = &rest[open..];
        let Some(close) = rest.find("</p>") else {
            break;
        };
        let paragraph = &rest[..close];
        rest = &rest[close + 4..];
        let Some(head_end) = paragraph.find('>') else {
            continue;
        };
        let Some(start_ms) = attribute(&paragraph[..head_end], "begin").and_then(parse_time) else {
            continue;
        };
        let mut words = Vec::new();
        let mut body = &paragraph[head_end + 1..];
        let mut depth_skip = 0usize;
        while let Some(at) = body.find('<') {
            let after = &body[at..];
            let Some(tag_end) = after.find('>') else {
                break;
            };
            let tag = &after[..=tag_end];
            body = &after[tag_end + 1..];
            if tag.starts_with("</span") {
                depth_skip = depth_skip.saturating_sub(1);
                continue;
            }
            if !tag.starts_with("<span") {
                continue;
            }
            if depth_skip > 0 || tag.contains("x-bg") {
                depth_skip += 1;
                continue;
            }
            let (Some(start), Some(end)) = (
                attribute(tag, "begin").and_then(parse_time),
                attribute(tag, "end").and_then(parse_time),
            ) else {
                continue;
            };
            let text_end = body.find('<').unwrap_or(body.len());
            let text = unescape(&body[..text_end]);
            if !text.trim().is_empty() {
                words.push(Word {
                    start_ms: start,
                    end_ms: end.max(start),
                    text: text.trim().to_string(),
                });
            }
        }
        if !words.is_empty() {
            lines.push(TimedLine { start_ms, words });
        }
    }
    lines
}

/// Letters and digits only, lower case.
fn letters(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Each character of `text` with when it is sung, from a timed line with
/// the same letters; `None` when the letters differ. A letter takes its
/// share of its word's time; anything between letters takes the time of
/// the letter before it.
pub fn char_times(text: &str, timed: &TimedLine) -> Option<Vec<(u32, u32)>> {
    let mut timeline: Vec<(u32, u32)> = Vec::new();
    for word in &timed.words {
        let count = letters(&word.text).len();
        let span = word.end_ms.saturating_sub(word.start_ms);
        for at in 0..count {
            let from = word.start_ms + span * at as u32 / count.max(1) as u32;
            let to = word.start_ms + span * (at as u32 + 1) / count.max(1) as u32;
            timeline.push((from, to));
        }
    }
    let shown: Vec<char> = text.chars().collect();
    let shown_letters = shown
        .iter()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .count();
    let timed_letters: usize = timed
        .words
        .iter()
        .map(|word| letters(&word.text).len())
        .sum();
    if shown_letters == 0
        || shown_letters != timed_letters
        || letters(text) != timed_letters_of(timed)
    {
        return None;
    }
    let mut times = Vec::with_capacity(shown.len());
    let mut next = timeline.into_iter();
    let mut last = (timed.start_ms, timed.start_ms);
    for c in shown {
        if c.is_alphanumeric() {
            // A letter that lower-cases to several takes one slot each.
            for _ in 0..c.to_lowercase().count() {
                last = next.next().unwrap_or(last);
            }
        }
        times.push(last);
    }
    Some(times)
}

fn timed_letters_of(timed: &TimedLine) -> Vec<char> {
    timed
        .words
        .iter()
        .flat_map(|word| letters(&word.text))
        .collect()
}

/// Character timings for every shown line, from the timed lines whose
/// letters match and which start near it.
pub fn match_lines(shown: &[(Option<u32>, String)], timed: &[TimedLine]) -> Vec<Option<CharTimes>> {
    shown
        .iter()
        .map(|(at, text)| {
            let at = (*at)?;
            // The nearest line with the same letters: a chorus repeats.
            let mut near: Vec<&TimedLine> = timed
                .iter()
                .filter(|line| line.start_ms.abs_diff(at) <= MAX_START_GAP_MS)
                .collect();
            near.sort_by_key(|line| line.start_ms.abs_diff(at));
            near.into_iter()
                .find_map(|line| char_times(text, line))
                .map(Arc::new)
        })
        .collect()
}

// ---- lookup ----------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    lines: Option<Vec<TimedLine>>,
}

pub fn cache_dir(lyrics_cache: &Path) -> PathBuf {
    lyrics_cache.join("words")
}

/// The song's word-timed lines, from the cache or Unison. `isrc` is
/// Spotify's, when known.
pub async fn fetch(
    http: &reqwest::Client,
    cache_dir: &Path,
    request: &Request,
    isrc: Option<&str>,
) -> Result<Option<Vec<TimedLine>>, String> {
    let Some(id) = request
        .uri
        .strip_prefix("spotify:track:")
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
    else {
        return Ok(None);
    };
    let path = cache_dir.join(format!("{id}.json"));
    if let Some(cached) = read_cache(&path) {
        log::debug!("word timing for {}: from the cache", request.uri);
        return Ok(cached);
    }
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(isrc) = isrc {
        query.push(("isrc", isrc.to_string()));
    }
    query.push(("song", crate::lyrics::clean_title(&request.title)));
    query.push(("artist", crate::lyrics::clean_artist(&request.artist)));
    if request.duration_ms > 0 {
        query.push(("duration", (request.duration_ms / 1000).to_string()));
    }
    log::info!("word timing for {}: asking Unison", request.uri);
    let response = http
        .get(API)
        .query(&query)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| format!("cannot reach Unison: {}", error.without_url()))?;
    let status = response.status();
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(format!("Unison answered {status}"));
    }
    let json: serde_json::Value = response.json().await.unwrap_or_default();
    let lines = json
        .pointer("/data")
        .filter(|data| {
            data.get("syncType").and_then(|kind| kind.as_str()) == Some("richsync")
                && data.get("format").and_then(|kind| kind.as_str()) == Some("ttml")
        })
        .and_then(|data| data.get("lyrics")?.as_str())
        .map(parse_ttml)
        .filter(|lines| !lines.is_empty());
    write_cache(&path, &lines);
    Ok(lines)
}

fn read_cache(path: &Path) -> Option<Option<Vec<TimedLine>>> {
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
    let lifetime = if cached.lines.is_some() {
        FOUND_LIFETIME
    } else {
        MISSING_LIFETIME
    };
    (age < lifetime).then_some(cached.lines)
}

fn write_cache(path: &Path, lines: &Option<Vec<TimedLine>>) {
    let Ok(bytes) = serde_json::to_vec(&Cached {
        version: CACHE_VERSION,
        lines: lines.clone(),
    }) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = fastframe_file::write(path, bytes) {
        log::debug!("word timing cache write failed: {error}");
    }
}

/// What the app holds: the playing song's word timing, by shown line.
#[derive(Default)]
pub struct State {
    pub uri: Option<String>,
    /// `None` while asking; `Some(None)` when Unison has none.
    pub lines: Option<Option<Vec<TimedLine>>>,
    /// Per shown line, each character's time; rebuilt when the lyrics or
    /// the answer change.
    pub matched: Vec<Option<CharTimes>>,
    /// The lyrics the matches were made for, by their first and last line.
    pub matched_for: Option<(String, usize)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTML: &str = r#"<tt><body><div>
      <p begin="0:02.250" end="0:03.900"><span begin="0:02.250" end="0:02.420">I'm</span> <span begin="0:02.420" end="0:02.670">a</span> <span begin="0:02.670" end="0:03.130">fire</span> <span begin="0:03.130" end="0:03.900">starter</span></p>
      <p begin="0:03.910" end="0:05.790"><span begin="0:03.910" end="0:04.130">Work</span> <span begin="0:04.130" end="0:04.560">a</span> <span ttm:role="x-bg"><span begin="0:04.200" end="0:04.400">(ooh)</span></span> <span begin="0:04.560" end="0:04.880">lil&apos;</span> <span begin="0:04.880" end="0:05.790">harder</span></p>
    </div></body></tt>"#;

    #[test]
    fn ttml_words_and_their_times_are_read() {
        let lines = parse_ttml(TTML);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start_ms, 2_250);
        assert_eq!(lines[0].words[2].text, "fire");
        assert_eq!(lines[0].words[2].start_ms, 2_670);
        let second: Vec<&str> = lines[1]
            .words
            .iter()
            .map(|word| word.text.as_str())
            .collect();
        assert_eq!(
            second,
            vec!["Work", "a", "lil'", "harder"],
            "background vocals are left out"
        );
    }

    #[test]
    fn shown_lines_get_their_letters_timed() {
        let timed = parse_ttml(TTML);
        let shown = vec![
            (Some(2_200), "I’m a fire starter".to_string()),
            (Some(3_900), "Work a lil harder".to_string()),
            (Some(9_000), "Something else entirely".to_string()),
        ];
        let matched = match_lines(&shown, &timed);
        let first = matched[0].as_ref().unwrap();
        // "f" of "fire" is the 7th character: I ’ m _ a _ f
        assert_eq!(first[6].0, 2_670);
        assert!(
            matched[1].is_some(),
            "an apostrophe more or less does not matter"
        );
        assert!(matched[2].is_none(), "nothing near with the same letters");
        assert_eq!(parse_time("1:02.5"), Some(62_500));
        assert_eq!(parse_time("12.25s"), Some(12_250));
    }
}
