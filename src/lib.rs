//! Spotifast's internals, exposed so diagnostics and tests can reach them.

pub mod account_cache;
pub mod api;
pub mod app;
#[cfg(target_os = "linux")]
pub mod appearance;
pub mod auth;
pub mod autoscroll;
pub mod backend;
pub mod bidi;
pub mod cast;
pub mod credentials;
#[cfg(any(test, feature = "demo"))]
pub mod demo;
pub mod device_history;
pub mod emoji;
pub mod eq;
/// Fork: artists' genres for the player bar, from MusicBrainz and Last.fm.
pub mod genres;
pub mod history;
pub mod http;
#[cfg(target_os = "linux")]
pub mod hyprland;
pub mod i18n;
pub mod images;
pub mod liked;
pub mod limiter;
pub mod link;
pub mod local_tags;
pub mod lyrics;
/// Fork: AI pronunciation and translation under the lyrics.
pub mod lyrics_ai;
/// Fork: the lyrics page's settings and per-song lyrics offsets.
pub mod lyrics_page;
/// Fork: music videos behind full-screen lyrics.
pub mod lyrics_video;
#[cfg(target_os = "macos")]
pub mod mac_links;
#[cfg(target_os = "macos")]
pub mod mac_menu;
#[cfg(target_os = "macos")]
pub mod mac_notch;
#[cfg(target_os = "macos")]
pub mod mac_touchbar_crash_guard;
pub mod made_for_you;
pub mod milkdrop;
pub mod model;
#[cfg(feature = "mods")]
pub mod mods;
pub mod motion;
pub mod new_releases;
pub mod notch;
pub mod opener;
pub mod paths;
pub mod player;
pub mod playlist_cover;
pub mod power;
pub mod prerelease;
pub mod resample;
pub mod seekwave;
pub mod session_reads;
pub mod settings;
pub mod single_instance;
pub mod sink;
pub mod skin;
pub mod skin_previews;
pub mod soloist;
pub mod sonos;
mod storage;
pub mod system_fonts;
pub mod theme;
pub mod thumbbar;
pub mod ui;
pub mod updates;
pub mod util;
pub mod vis;
pub mod winamp;
pub mod window;
/// Fork: word timing from Unison for the word-by-word light.
pub mod word_timing;
pub mod zeroconf;
