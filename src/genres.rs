//! Fork: an artist's genres for the player bar's genre line.
//!
//! Spotify no longer lists genres for apps outside its own clients: the Web
//! API answers `"genres": []` for every artist, and the session's artist
//! metadata dropped the field. So the genres come from
//! [MusicBrainz](https://musicbrainz.org), which links many artists to their
//! Spotify ID. When it has no genres, [Wikidata](https://www.wikidata.org)
//! is asked by the same Spotify ID, then MusicBrainz by the artist's name
//! (only for an unmistakable match), and, when none has any and the listener
//! saved a key, [Last.fm](https://www.last.fm)'s top tags for the name.
//! Answers are kept on disk, so an artist is looked up about once a month.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const MUSICBRAINZ: &str = "https://musicbrainz.org/ws/2";
const LASTFM: &str = "https://ws.audioscrobbler.com/2.0/";
const WIKIDATA: &str = "https://query.wikidata.org/sparql";
/// MusicBrainz asks for a contact in the User-Agent and at most one request a
/// second from each client.
const MUSICBRAINZ_AGENT: &str = concat!(
    "Spotifast/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/watdafawx/spotifast )"
);
const MUSICBRAINZ_SPACING: Duration = Duration::from_millis(1100);
const FOUND_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MISSING_LIFETIME: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// 2: misses are asked again, now that artists MusicBrainz has no Spotify
/// link or genres for are also found by name.
const CACHE_VERSION: u32 = 2;
/// The most genres kept for an artist; the player bar shows three.
const KEEP: usize = 5;
/// Last.fm tags below this share of the top tag's count are noise.
const LASTFM_MIN_COUNT: u64 = 20;
/// Last.fm tags that say nothing about the sound.
const LASTFM_IGNORED: &[&str] = &[
    "seen live",
    "favorites",
    "favourites",
    "favorite",
    "my favorite",
    "love",
    "loved",
    "awesome",
    "beautiful",
    "male vocalists",
    "female vocalists",
    "male vocalist",
    "female vocalist",
    "singer-songwriter",
    "under 2000 listeners",
    "spotify",
    "albums i own",
];

/// One artist to look up.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The Spotify artist ID (base62).
    pub id: String,
    pub name: String,
}

/// Keeps MusicBrainz requests a second apart across every lookup.
#[derive(Clone, Default)]
pub struct Gate(std::sync::Arc<tokio::sync::Mutex<Option<Instant>>>);

impl Gate {
    async fn wait(&self) -> tokio::sync::MutexGuard<'_, Option<Instant>> {
        let mut last = self.0.lock().await;
        if let Some(at) = *last {
            let since = at.elapsed();
            if since < MUSICBRAINZ_SPACING {
                tokio::time::sleep(MUSICBRAINZ_SPACING - since).await;
            }
        }
        *last = Some(Instant::now());
        last
    }
}

#[derive(Deserialize, Serialize)]
struct Cached {
    version: u32,
    genres: Vec<String>,
    /// Whether Last.fm was asked: a miss without it is asked again once a
    /// key is saved.
    #[serde(default)]
    lastfm: bool,
}

pub fn cache_dir(cache: &Path) -> PathBuf {
    cache.join("genres")
}

/// The artist's genres, most fitting first, lower case as the sources give
/// them. Empty when no source knows the artist. An error is worth asking
/// again later; a miss is not.
pub async fn fetch(
    http: &reqwest::Client,
    gate: &Gate,
    cache_dir: &Path,
    request: &Request,
    lastfm_key: Option<&str>,
) -> Result<Vec<String>, String> {
    if request.id.is_empty() || !request.id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(Vec::new());
    }
    let lastfm_key = lastfm_key.map(str::trim).filter(|key| !key.is_empty());
    let path = cache_dir.join(format!("{}.json", request.id));
    if let Some(cached) = read_cache(&path, lastfm_key.is_some()) {
        return Ok(cached);
    }
    // Sources in order of trust: MusicBrainz through the Spotify link, then
    // Wikidata through the same Spotify ID, then MusicBrainz by name, then
    // Last.fm when the listener saved a key.
    let linked = musicbrainz_linked(http, gate, &request.id).await?;
    let mut genres = linked.genres;
    if genres.is_empty() {
        // Optional: a Wikidata outage must not hide what MusicBrainz knows.
        match wikidata(http, &request.id).await {
            Ok(found) => genres = found,
            Err(error) => log::debug!("genres: {error}"),
        }
    }
    if genres.is_empty() {
        genres = musicbrainz_named(http, gate, &request.name, linked.artist.as_deref()).await?;
    }
    let mut asked_lastfm = false;
    if genres.is_empty()
        && let Some(key) = lastfm_key
        && !request.name.trim().is_empty()
    {
        asked_lastfm = true;
        genres = lastfm(http, key, &request.name).await?;
    }
    log::info!(
        "genres for artist {}: {} found{}",
        request.id,
        genres.len(),
        if asked_lastfm { " (Last.fm)" } else { "" }
    );
    write_cache(&path, &genres, asked_lastfm || lastfm_key.is_some());
    Ok(genres)
}

/// What the Spotify link found: the MusicBrainz artist, if linked, and its
/// genres.
struct Linked {
    artist: Option<String>,
    genres: Vec<String>,
}

async fn musicbrainz_linked(
    http: &reqwest::Client,
    gate: &Gate,
    id: &str,
) -> Result<Linked, String> {
    let resource = format!("https://open.spotify.com/artist/{id}");
    let link = {
        let _turn = gate.wait().await;
        get_json(
            http.get(format!("{MUSICBRAINZ}/url"))
                .query(&[
                    ("resource", resource.as_str()),
                    ("inc", "artist-rels"),
                    ("fmt", "json"),
                ])
                .header("User-Agent", MUSICBRAINZ_AGENT),
            "MusicBrainz",
        )
        .await?
    };
    let artist = link.and_then(|link| musicbrainz_artist(&link));
    let genres = match &artist {
        Some(artist) => musicbrainz_artist_genres(http, gate, artist).await?,
        None => Vec::new(),
    };
    Ok(Linked { artist, genres })
}

/// Finds the artist by name, when exactly one is an unmistakable match.
/// `already` is the artist the Spotify link gave, not worth asking twice.
async fn musicbrainz_named(
    http: &reqwest::Client,
    gate: &Gate,
    name: &str,
    already: Option<&str>,
) -> Result<Vec<String>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let search = {
        let _turn = gate.wait().await;
        get_json(
            http.get(format!("{MUSICBRAINZ}/artist"))
                .query(&[
                    ("query", format!("artist:\"{}\"", escape_query(name)).as_str()),
                    ("limit", "5"),
                    ("fmt", "json"),
                ])
                .header("User-Agent", MUSICBRAINZ_AGENT),
            "MusicBrainz",
        )
        .await?
    };
    let Some(found) = search.and_then(|search| musicbrainz_match(&search, name)) else {
        return Ok(Vec::new());
    };
    if already == Some(found.as_str()) {
        return Ok(Vec::new());
    }
    musicbrainz_artist_genres(http, gate, &found).await
}

/// The genres Wikidata lists for the artist with this Spotify ID. The ID is
/// matched exactly, so no wrong artist can answer.
async fn wikidata(http: &reqwest::Client, id: &str) -> Result<Vec<String>, String> {
    // `id` was checked to be letters and digits only, so it cannot break out
    // of the quoted string.
    let query = format!(
        "SELECT DISTINCT ?gLabel WHERE {{ ?a wdt:P1902 \"{id}\"; wdt:P136 ?g.          SERVICE wikibase:label {{ bd:serviceParam wikibase:language \"en\". }} }}"
    );
    let found = get_json(
        http.get(WIKIDATA)
            .query(&[("query", query.as_str()), ("format", "json")])
            .header("User-Agent", MUSICBRAINZ_AGENT),
        "Wikidata",
    )
    .await?;
    Ok(found
        .map(|found| wikidata_genres(&found))
        .unwrap_or_default())
}

/// Wikidata's genre labels as the player bar wants them: "pop music" as
/// "pop", "K-pop" as "k-pop".
fn wikidata_genres(found: &serde_json::Value) -> Vec<String> {
    let mut genres: Vec<String> = Vec::new();
    for binding in found
        .pointer("/results/bindings")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(label) = binding.pointer("/gLabel/value").and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let label = label.trim().to_lowercase();
        let label = label
            .strip_suffix(" music")
            .filter(|rest| !rest.is_empty())
            .unwrap_or(&label)
            .to_string();
        // An unlabelled item comes back as its Q-number.
        let unlabelled = label.starts_with('q') && label[1..].chars().all(|c| c.is_ascii_digit());
        if !label.is_empty() && !unlabelled && !genres.contains(&label) {
            genres.push(label);
        }
    }
    genres.truncate(KEEP);
    genres
}

/// A MusicBrainz artist's genres, or its tags when it has no genres.
async fn musicbrainz_artist_genres(
    http: &reqwest::Client,
    gate: &Gate,
    artist: &str,
) -> Result<Vec<String>, String> {
    let _turn = gate.wait().await;
    let found = get_json(
        http.get(format!("{MUSICBRAINZ}/artist/{artist}"))
            .query(&[("inc", "genres tags"), ("fmt", "json")])
            .header("User-Agent", MUSICBRAINZ_AGENT),
        "MusicBrainz",
    )
    .await?;
    Ok(found
        .map(|found| {
            let genres = musicbrainz_genres(&found);
            if genres.is_empty() {
                musicbrainz_tags(&found)
            } else {
                genres
            }
        })
        .unwrap_or_default())
}

/// Quotes and backslashes would end the phrase in a search query.
fn escape_query(name: &str) -> String {
    name.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The one artist in a name search whose name is exactly `name` and whose
/// score is near perfect. Two such artists share the name, so neither is
/// safe to pick.
fn musicbrainz_match(search: &serde_json::Value, name: &str) -> Option<String> {
    let mut matches = search
        .get("artists")?
        .as_array()?
        .iter()
        .filter(|artist| {
            artist
                .get("score")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|score| score >= 95)
                && artist
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|found| found.trim().eq_ignore_ascii_case(name))
        })
        .filter_map(|artist| artist.get("id")?.as_str())
        .filter(|id| id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    let only = matches.next()?;
    matches.next().is_none().then(|| only.to_string())
}

async fn lastfm(http: &reqwest::Client, key: &str, name: &str) -> Result<Vec<String>, String> {
    let found = get_json(
        http.get(LASTFM).query(&[
            ("method", "artist.gettoptags"),
            ("artist", name),
            ("autocorrect", "1"),
            ("api_key", key),
            ("format", "json"),
        ]),
        "Last.fm",
    )
    .await?;
    Ok(found.map(|found| lastfm_genres(&found)).unwrap_or_default())
}

/// The JSON answer, `None` when the service does not know the thing asked
/// for. Errors never include the URL, which can hold the Last.fm key.
async fn get_json(
    request: reqwest::RequestBuilder,
    service: &str,
) -> Result<Option<serde_json::Value>, String> {
    let response = request
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| format!("cannot reach {service}: {}", error.without_url()))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("{service} answered {status}"));
    }
    response.json().await.map(Some).map_err(|error| {
        format!(
            "{service} sent an unreadable answer: {}",
            error.without_url()
        )
    })
}

/// The MusicBrainz artist a Spotify artist URL belongs to.
fn musicbrainz_artist(link: &serde_json::Value) -> Option<String> {
    link.get("relations")?
        .as_array()?
        .iter()
        .find_map(|relation| relation.pointer("/artist/id")?.as_str())
        .filter(|id| id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .map(str::to_string)
}

/// An artist's MusicBrainz genres, most voted first.
fn musicbrainz_genres(artist: &serde_json::Value) -> Vec<String> {
    musicbrainz_votes(artist, "genres", 1)
}

/// An artist's MusicBrainz tags that read as genres, for artists without
/// any. Tags are looser: a tag needs a second vote.
fn musicbrainz_tags(artist: &serde_json::Value) -> Vec<String> {
    musicbrainz_votes(artist, "tags", 2)
        .into_iter()
        .filter(|tag| {
            !LASTFM_IGNORED.contains(&tag.as_str()) && !tag.starts_with(|c: char| c.is_ascii_digit())
        })
        .collect()
}

fn musicbrainz_votes(artist: &serde_json::Value, field: &str, min: u64) -> Vec<String> {
    let mut genres: Vec<(u64, String)> = artist
        .get(field)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| {
            let count = genre.get("count")?.as_u64()?;
            let name = genre.get("name")?.as_str()?.trim().to_lowercase();
            (count >= min && !name.is_empty()).then_some((count, name))
        })
        .collect();
    // Stable, so equal votes keep MusicBrainz's order.
    genres.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    genres
        .into_iter()
        .map(|(_, name)| name)
        .take(KEEP)
        .collect()
}

/// An artist's Last.fm top tags that read as genres, most used first.
fn lastfm_genres(found: &serde_json::Value) -> Vec<String> {
    found
        .pointer("/toptags/tag")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| {
            let count = match tag.get("count")? {
                serde_json::Value::Number(count) => count.as_u64()?,
                serde_json::Value::String(count) => count.parse().ok()?,
                _ => return None,
            };
            let name = tag.get("name")?.as_str()?.trim().to_lowercase();
            (count >= LASTFM_MIN_COUNT
                && !name.is_empty()
                && !LASTFM_IGNORED.contains(&name.as_str())
                // Decades and years, such as "80s" or "2010".
                && !name.starts_with(|c: char| c.is_ascii_digit()))
            .then_some(name)
        })
        .take(KEEP)
        .collect()
}

fn read_cache(path: &Path, have_lastfm: bool) -> Option<Vec<String>> {
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
    if cached.genres.is_empty() {
        // A key saved since the miss may find them.
        (age < MISSING_LIFETIME && (cached.lastfm || !have_lastfm)).then_some(cached.genres)
    } else {
        (age < FOUND_LIFETIME).then_some(cached.genres)
    }
}

fn write_cache(path: &Path, genres: &[String], lastfm: bool) {
    let Ok(bytes) = serde_json::to_vec(&Cached {
        version: CACHE_VERSION,
        genres: genres.to_vec(),
        lastfm,
    }) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = fastframe_file::write(path, bytes) {
        log::debug!("genre cache write failed: {error}");
    }
}

/// "latin pop" as "Latin Pop" and "r&b" as "R&B", for the player bar.
pub fn display(genre: &str) -> String {
    genre
        .split(' ')
        .map(|word| {
            if word.contains('&') && word.len() <= 4 {
                return word.to_uppercase();
            }
            word.split('-')
                .map(|part| {
                    let mut chars = part.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect()
                    })
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_spotify_link_finds_its_musicbrainz_artist() {
        let link = json!({
            "resource": "https://open.spotify.com/artist/3TVXtAsR1Inumwj472S9r4",
            "relations": [{"type": "free streaming", "artist": {
                "id": "9fff2f8a-21e6-47de-a2b8-7f449929d43f", "name": "Drake"
            }}]
        });
        assert_eq!(
            musicbrainz_artist(&link).as_deref(),
            Some("9fff2f8a-21e6-47de-a2b8-7f449929d43f")
        );
        assert_eq!(musicbrainz_artist(&json!({"relations": []})), None);
        assert_eq!(
            musicbrainz_artist(&json!({"relations": [{"artist": {"id": "../x?"}}]})),
            None
        );
    }

    #[test]
    fn musicbrainz_genres_come_most_voted_first() {
        let artist = json!({"genres": [
            {"name": "pop rap", "count": 9},
            {"name": "Hip Hop", "count": 22},
            {"name": "r&b", "count": 11},
            {"name": "unvoted", "count": 0},
        ]});
        assert_eq!(musicbrainz_genres(&artist), ["hip hop", "r&b", "pop rap"]);
        assert!(musicbrainz_genres(&json!({"genres": []})).is_empty());
        assert!(musicbrainz_genres(&json!({})).is_empty());
    }

    #[test]
    fn a_name_search_picks_only_an_unmistakable_match() {
        let id = "9fff2f8a-21e6-47de-a2b8-7f449929d43f";
        let one = json!({"artists": [
            {"id": id, "name": "BLACKPINK", "score": 100},
            {"id": "11111111-1111-1111-1111-111111111111", "name": "Blackpink Tribute", "score": 70},
        ]});
        assert_eq!(musicbrainz_match(&one, "Blackpink").as_deref(), Some(id));
        let two = json!({"artists": [
            {"id": id, "name": "KDA", "score": 100},
            {"id": "11111111-1111-1111-1111-111111111111", "name": "kda", "score": 98},
        ]});
        assert_eq!(musicbrainz_match(&two, "KDA"), None);
        let weak = json!({"artists": [{"id": id, "name": "KDA", "score": 80}]});
        assert_eq!(musicbrainz_match(&weak, "KDA"), None);
        assert_eq!(escape_query(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn wikidata_labels_read_as_genres() {
        let found = json!({"results": {"bindings": [
            {"gLabel": {"value": "hip-hop"}},
            {"gLabel": {"value": "pop music"}},
            {"gLabel": {"value": "K-pop"}},
            {"gLabel": {"value": "Q123456"}},
            {"gLabel": {"value": "Pop Music"}},
        ]}});
        assert_eq!(wikidata_genres(&found), ["hip-hop", "pop", "k-pop"]);
        assert!(wikidata_genres(&json!({})).is_empty());
    }

    #[test]
    fn musicbrainz_tags_need_two_votes_and_read_as_genres() {
        let artist = json!({"tags": [
            {"name": "k-pop", "count": 9},
            {"name": "seen live", "count": 5},
            {"name": "american", "count": 1},
            {"name": "2010s", "count": 4},
        ]});
        assert_eq!(musicbrainz_tags(&artist), ["k-pop"]);
    }

    #[test]
    fn lastfm_tags_keep_only_genres() {
        let found = json!({"toptags": {"tag": [
            {"name": "latin pop", "count": 100},
            {"name": "seen live", "count": 80},
            {"name": "Reggaeton", "count": "60"},
            {"name": "female vocalists", "count": 50},
            {"name": "2010s", "count": 40},
            {"name": "pop", "count": 30},
            {"name": "mexican", "count": 5},
        ]}});
        assert_eq!(lastfm_genres(&found), ["latin pop", "reggaeton", "pop"]);
        assert!(lastfm_genres(&json!({"error": 6, "message": "not found"})).is_empty());
    }

    #[test]
    fn genres_read_as_titles() {
        assert_eq!(display("latin pop"), "Latin Pop");
        assert_eq!(display("r&b"), "R&B");
        assert_eq!(display("contemporary r&b"), "Contemporary R&B");
        assert_eq!(display("k-pop"), "K-Pop");
        assert_eq!(display("hip hop"), "Hip Hop");
        assert_eq!(display("Pop"), "Pop");
    }

    #[test]
    fn a_miss_is_asked_again_once_a_lastfm_key_is_saved() {
        let root = std::env::temp_dir().join(format!("spotifast-genres-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let found = root.join("found.json");
        write_cache(&found, &["pop".into()], false);
        assert_eq!(read_cache(&found, false), Some(vec!["pop".to_string()]));
        assert_eq!(read_cache(&found, true), Some(vec!["pop".to_string()]));

        let missing = root.join("missing.json");
        write_cache(&missing, &[], false);
        assert_eq!(read_cache(&missing, false), Some(Vec::new()));
        assert_eq!(read_cache(&missing, true), None);
        write_cache(&missing, &[], true);
        assert_eq!(read_cache(&missing, true), Some(Vec::new()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keys_saved_before_lastfm_still_load() {
        let keys: crate::lyrics_ai::Keys =
            serde_json::from_str(r#"{"gemini":"g","openai":""}"#).unwrap();
        assert!(keys.lastfm.is_empty());
        assert!(!keys.present().lastfm);
    }

    /// Asks the real MusicBrainz: `cargo test genres -- --ignored`.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn musicbrainz_knows_drake() {
        let http = reqwest::Client::new();
        let root =
            std::env::temp_dir().join(format!("spotifast-genres-live-{}", std::process::id()));
        let request = Request {
            id: "3TVXtAsR1Inumwj472S9r4".into(),
            name: "Drake".into(),
        };
        let gate = Gate::default();
        let genres = fetch(&http, &gate, &root, &request, None).await.unwrap();
        assert!(genres.iter().any(|genre| genre == "hip hop"), "{genres:?}");
        // The second time comes from the disk.
        let started = Instant::now();
        assert_eq!(
            fetch(&http, &gate, &root, &request, None).await.unwrap(),
            genres
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unusable_id_asks_nobody() {
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let root = std::env::temp_dir().join(format!("spotifast-genres-id-{}", std::process::id()));
        let request = Request {
            id: "../../etc".into(),
            name: "x".into(),
        };
        assert_eq!(
            fetch(&http, &Gate::default(), &root, &request, None).await,
            Ok(Vec::new())
        );
        assert!(!root.exists());
    }
}
