//! Fork: everything this fork adds to Spotifast.
//!
//! Upstream's files reach this code only through one-line hooks, listed in
//! FORK.md, so the fork rebases onto upstream without conflicts. New fork
//! work goes here (or in a `fork` child module of the upstream file whose
//! private items it needs), never inline in upstream code.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Artists' genres for the player bar, from MusicBrainz and Last.fm.
pub mod genres;
/// Windows 11 acrylic glass behind the main window.
#[cfg(windows)]
pub mod glass;
/// AI pronunciation and translation under the lyrics.
pub mod lyrics_ai;
/// The lyrics page's settings and per-song lyrics offsets.
pub mod lyrics_page;
/// Music videos behind full-screen lyrics.
pub mod lyrics_video;
/// Spicetify-style mods in a webview child process.
#[cfg(feature = "mods")]
pub mod mods;
/// The waveform seek bar's picture of the playing song.
pub mod seekwave;
/// Word timing from Unison for the word-by-word light.
pub mod word_timing;

/// What is known of one artist's genres, for the player bar.
#[derive(Clone, Debug, PartialEq)]
pub enum ArtistGenres {
    Loading,
    Known(Vec<String>),
    /// The request failed then; it is asked again after [`GENRES_RETRY`].
    Failed(Instant),
}

/// How long a failed genre request waits before the next try.
pub const GENRES_RETRY: std::time::Duration = std::time::Duration::from_secs(300);

/// The fork's part of [`crate::app::App`], its one field `fork`.
#[derive(Default)]
pub struct State {
    /// Genres by artist id for the player bar's genre line. Kept across
    /// account changes, since genres are the same for everyone.
    pub artist_genres: HashMap<String, ArtistGenres>,
    /// Pronunciation and translation under the lyrics.
    pub lyrics_ai: lyrics_ai::State,
    /// The music video behind full-screen lyrics.
    pub lyrics_video: lyrics_video::State,
    /// The native lyrics page holds the main area.
    pub lyrics_page_open: bool,
    /// Word timing for the word-by-word light.
    pub word_timing: word_timing::State,
    /// When scrolling by hand last paused following.
    pub lyrics_paused_at: Option<Instant>,
    /// What the waveform seek bar last copied of the playing song.
    pub seek_wave_view: seekwave::View,
}

/// The fork's settings, flattened into `settings.json` beside upstream's,
/// so files written before the fork kept them in their own struct still
/// read the same.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// A dim line of the playing song's artists' genres under the artist
    /// names in the player bar. Older files without it show the line.
    pub player_bar_genres: bool,
    /// Draw the seek bar as the song's waveform: how loud it is along its
    /// whole length, the played part bright. Worked out from the sound
    /// itself and kept per song in the cache.
    pub seek_waveform: bool,
    /// The Library collapsed to a strip of covers, names on hover.
    pub sidebar_collapsed: bool,
    /// Pronunciation and translation under the lyrics. The keys are in the
    /// credential store, never here.
    pub lyrics_ai: lyrics_ai::AiSettings,
    /// A community-picked music video behind full-screen lyrics, where
    /// libmpv is installed.
    pub lyrics_video: bool,
    /// The lyrics page's look and per-song lyrics offsets.
    pub lyrics_page: lyrics_page::PageSettings,
    /// The webview mod (ivLyrics) and its page, in builds with mods. Off by
    /// default; the native lyrics page replaces it.
    pub mods_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            player_bar_genres: true,
            seek_waveform: false,
            sidebar_collapsed: false,
            lyrics_ai: lyrics_ai::AiSettings::default(),
            lyrics_video: true,
            lyrics_page: lyrics_page::PageSettings::default(),
            mods_enabled: false,
        }
    }
}

/// The fork's actions, carried by `Action::Fork`.
#[derive(Clone, Debug)]
pub enum Action {
    /// Open or close the native lyrics page.
    ToggleLyricsPage,
    /// Pick the playing song's video, or none, or the search's.
    ChooseLyricsVideo(lyrics_video::Choice),
    /// Ask again for the lyrics' extra lines after a failure.
    RetryLyricsExtras,
    /// Ask which AI keys are saved.
    LoadAiKeys,
    /// List the models a provider offers at an endpoint.
    ListAiModels {
        provider: lyrics_ai::Provider,
        base_url: String,
    },
    /// Save an AI provider's key; empty removes it.
    SaveAiKey {
        provider: lyrics_ai::Provider,
        key: lyrics_ai::SecretKey,
    },
    /// Save the Last.fm key genres use; empty removes it.
    SaveLastFmKey(lyrics_ai::SecretKey),
    /// Collapse the Library to a strip of covers, or open it again.
    SetLibraryCollapsed(bool),
}

/// The fork's backend events, carried by `Event::Fork`.
pub enum Event {
    /// An artist's genres, most fitting first; an error is worth asking
    /// again later.
    ArtistGenres {
        id: String,
        result: Result<Vec<String>, String>,
    },
    /// The extra lines for the lyrics request with this id.
    LyricsExtras {
        id: String,
        result: Result<lyrics_ai::Extras, lyrics_ai::Error>,
    },
    /// Which AI keys are saved, or why storing one failed.
    AiKeys(Result<lyrics_ai::KeysPresent, String>),
    /// The models a provider offers at an endpoint.
    AiModels {
        provider: lyrics_ai::Provider,
        base_url: String,
        result: Result<Vec<lyrics_ai::ModelChoice>, String>,
    },
    /// A song's word-timed lines, `None` when there are none.
    WordTiming {
        uri: String,
        result: Result<Option<Vec<word_timing::TimedLine>>, String>,
    },
    /// The music video for a song, `None` when it has none.
    LyricsVideo {
        uri: String,
        result: Result<Option<lyrics_video::VideoInfo>, String>,
    },
}

/// The fork's backend commands, carried by `Command::Fork`.
pub enum Command {
    /// Pronunciation and translation lines for a song's lyrics.
    LyricsExtras(Box<lyrics_ai::Request>),
    /// Word timing for a song.
    FindWordTiming(Box<word_timing::Request>),
    /// Find the music video for a song.
    FindLyricsVideo(Box<lyrics_video::Request>),
    /// The listener picks a song's video, or none, or the search's.
    ChooseLyricsVideo(Box<lyrics_video::Request>, lyrics_video::Choice),
    /// An artist's genres for the player bar.
    ArtistGenres(genres::Request),
    /// Save the Last.fm key genres use; an empty key removes it.
    SetLastFmKey(lyrics_ai::SecretKey),
    /// Read which AI keys are saved, for the settings page.
    LoadAiKeys,
    /// List the models a provider offers.
    ListAiModels {
        provider: lyrics_ai::Provider,
        base_url: String,
    },
    /// Save a provider's AI key; an empty key removes it.
    SetAiKey {
        provider: lyrics_ai::Provider,
        key: lyrics_ai::SecretKey,
    },
    /// Internal: the AI keys were read from the credential store.
    AiKeysLoaded {
        lease: crate::credentials::Lease,
        result: Result<crate::credentials::Loaded, crate::credentials::Error>,
    },
    /// A Spotify request from a mod.
    #[cfg(feature = "mods")]
    Mods(Box<mods::proxy::Request>),
}

impl crate::paths::AppDirs {
    /// Artists' genres, a few dozen bytes each.
    pub fn genre_cache_dir(&self) -> std::path::PathBuf {
        genres::cache_dir(&self.cache)
    }

    /// The waveform seek bar's pictures, about a kilobyte per song.
    pub fn waveform_cache_dir(&self) -> std::path::PathBuf {
        self.cache.join("waveforms")
    }
}

/// Hook in the entry point: a `--mods-child` launch is the mods webview,
/// not the app. It runs and exits here; any other launch returns.
pub fn run_child() {
    #[cfg(feature = "mods")]
    if let Some(args) = mods::child::Args::parse() {
        std::process::exit(mods::child::run(args));
    }
}

/// Hook in the entry point once a window exists: the mods window's parent,
/// and on Windows the glass behind the main window.
pub fn attach_window(cc: &eframe::CreationContext<'_>, main: bool) {
    #[cfg(feature = "mods")]
    mods::attach_window(cc);
    #[cfg(windows)]
    if main {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(window) = handle.as_raw()
        {
            glass::frosted_backdrop(window.hwnd.get());
        }
    }
    let _ = (cc, main);
}

/// Hook in the entry point's window options: on Windows the main window is
/// see-through, over the acrylic blur [`glass::frosted_backdrop`] asks for.
pub fn window_viewport(viewport: egui::ViewportBuilder, main: bool) -> egui::ViewportBuilder {
    if cfg!(windows) && main {
        viewport.with_transparent(true)
    } else {
        viewport
    }
}

/// Every window clears to nothing on Windows, where the big window is
/// see-through too (glass themes).
pub const SEE_THROUGH: bool = cfg!(windows);
