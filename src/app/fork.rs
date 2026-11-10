//! Fork: the fork's part of [`App`]. `app.rs` calls in through one-line
//! hooks (FORK.md lists them); everything else the fork does to the app's
//! state lives here, as a child module so it can reach `App`'s private
//! fields.

use std::time::Instant;

use super::*;
use crate::fork::{self as fork_mod, ArtistGenres, GENRES_RETRY};

/// Hook in `App::new`: the waveform seek bar's cache and switch.
pub(super) fn init_tap(
    tap: &crate::vis::AudioTap,
    dirs: &crate::paths::AppDirs,
    settings: &crate::settings::Settings,
) {
    tap.seek_wave.set_dir(dirs.waveform_cache_dir());
    tap.seek_wave.set_enabled(settings.fork.seek_waveform);
}

/// Hook in `Event::UpdateChecked`: a repository with no release yet (the
/// fork, before its first one) answers 404, so there is nothing newer.
pub(super) fn update_checked<T, E: std::fmt::Display>(
    result: Result<Option<T>, E>,
) -> Result<Option<T>, E> {
    match result {
        Err(error) if error.to_string().ends_with("answered 404") => Ok(None),
        result => result,
    }
}

impl App {
    /// Hook after new lyrics arrive.
    pub(super) fn fork_lyrics_loaded(&mut self) {
        self.request_lyrics_extras();
        self.request_word_timing();
    }

    /// Hook after the playing song changes: the lyrics page shows lyrics
    /// without the panel flag, so it must refresh them too.
    pub(super) fn fork_track_changed(&mut self) {
        if self.fork.lyrics_page_open
            && !(self.show_lyrics_panel || self.lyrics_fullscreen.is_some())
        {
            self.request_lyrics();
        }
    }

    /// Hook at the top of `sync_media_controls`, once a frame.
    pub(super) fn fork_sync(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "mods")]
        crate::fork::mods::sync(self, ctx);
        // The music video behind full-screen lyrics.
        crate::fork::lyrics_video::sync(self, ctx);
    }

    /// Hook on a Web API artist: Spotify lists genres only for some apps;
    /// when it does, they spare the lookup elsewhere.
    pub(super) fn fork_artist(&mut self, id: &str, artist: &crate::api::models::Artist) {
        if !artist.genres.is_empty() {
            self.fork
                .artist_genres
                .insert(id.to_string(), ArtistGenres::Known(artist.genres.clone()));
        }
    }

    /// Hook before every action is applied. True when the fork has dealt
    /// with it and upstream's handling must not run.
    pub(super) fn fork_before_action(&mut self, action: &Action) -> bool {
        match action {
            // Going to a page leaves the lyrics page.
            Action::Open(_) => self.fork.lyrics_page_open = false,
            Action::Back => {
                // The lyrics page closes like a page of its own.
                if std::mem::take(&mut self.fork.lyrics_page_open) {
                    return true;
                }
                // The mod's page closes like a page of its own.
                #[cfg(feature = "mods")]
                if crate::fork::mods::close_page() {
                    return true;
                }
            }
            Action::LyricsLineShown(line) => {
                // Upstream keeps the line only for full screen; the lyrics
                // page needs it too.
                if self.fork.lyrics_page_open {
                    self.lyrics_line_shown = Some(*line);
                }
            }
            // Following picks up again shortly after the scroll.
            Action::PauseLyricsFollow => self.fork.lyrics_paused_at = Some(Instant::now()),
            Action::SettingsChanged => {
                self.request_lyrics_extras();
                self.request_word_timing();
            }
            _ => {}
        }
        false
    }

    /// `Action::Fork`.
    pub(super) fn fork_action(&mut self, action: fork_mod::Action) {
        use fork_mod::Action as A;
        match action {
            A::ChooseLyricsVideo(choice) => {
                if let Some(request) = crate::fork::lyrics_video::request_for(self) {
                    // Shown as loading until the backend answers.
                    self.fork.lyrics_video.uri = Some(request.uri.clone());
                    self.fork.lyrics_video.info = None;
                    self.backend
                        .send(Command::Fork(fork_mod::Command::ChooseLyricsVideo(
                            Box::new(request),
                            choice,
                        )));
                }
            }
            A::ToggleLyricsPage => {
                self.fork.lyrics_page_open = !self.fork.lyrics_page_open;
                if self.fork.lyrics_page_open {
                    self.lyrics_following = true;
                    self.lyrics_line_shown = None;
                    self.request_lyrics();
                }
            }
            A::RetryLyricsExtras => {
                self.fork.lyrics_ai.retry();
                self.request_lyrics_extras();
            }
            A::LoadAiKeys => self
                .backend
                .send(Command::Fork(fork_mod::Command::LoadAiKeys)),
            A::ListAiModels { provider, base_url } => {
                let wanted = (provider, base_url.clone());
                if self.fork.lyrics_ai.models_loading.as_ref() != Some(&wanted) {
                    self.fork.lyrics_ai.models_loading = Some(wanted);
                    self.fork.lyrics_ai.models_error = None;
                    self.backend
                        .send(Command::Fork(fork_mod::Command::ListAiModels {
                            provider,
                            base_url,
                        }));
                }
            }
            A::SaveLastFmKey(key) => self
                .backend
                .send(Command::Fork(fork_mod::Command::SetLastFmKey(key))),
            A::SaveAiKey { provider, key } => {
                self.backend
                    .send(Command::Fork(fork_mod::Command::SetAiKey { provider, key }));
            }
            A::SetLibraryCollapsed(collapsed) => {
                self.settings.fork.sidebar_collapsed = collapsed;
                self.mark_settings_dirty();
            }
        }
    }

    /// `Event::Fork`.
    pub(super) fn fork_event(&mut self, event: fork_mod::Event) {
        use fork_mod::Event as E;
        match event {
            E::WordTiming { uri, result } => {
                if self.fork.word_timing.uri.as_deref() == Some(uri.as_str()) {
                    self.fork.word_timing.lines = Some(result.ok().flatten());
                    self.fork.word_timing.matched_for = None;
                    self.match_word_timing();
                }
            }
            E::LyricsExtras { id, result } => self.receive_lyrics_extras(&id, result),
            E::LyricsVideo { uri, result } => {
                if self.fork.lyrics_video.uri.as_deref() == Some(uri.as_str()) {
                    // A failed lookup shows the cover, as no video does,
                    // and a quiet line in the lyrics page's Video panel.
                    self.fork.lyrics_video.search_failed = result.is_err();
                    self.fork.lyrics_video.info = Some(result.ok().flatten());
                }
            }
            E::ArtistGenres { id, result } => {
                let entry = match result {
                    Ok(genres) => ArtistGenres::Known(genres),
                    Err(_) => ArtistGenres::Failed(Instant::now()),
                };
                self.fork.artist_genres.insert(id, entry);
            }
            E::AiKeys(result) => match result {
                Ok(present) => {
                    // A new Last.fm key may find the genres MusicBrainz
                    // lacked, so the misses are asked again.
                    if present.lastfm && !self.fork.lyrics_ai.keys.is_some_and(|keys| keys.lastfm) {
                        self.fork.artist_genres.retain(|_, genres| {
                            !matches!(genres, ArtistGenres::Known(found) if found.is_empty())
                        });
                    }
                    self.fork.lyrics_ai.keys = Some(present);
                    self.fork.lyrics_ai.retry();
                    self.request_lyrics_extras();
                }
                Err(error) => self.fork.lyrics_ai.key_error = Some(error),
            },
            E::AiModels {
                provider,
                base_url,
                result,
            } => {
                if self.fork.lyrics_ai.models_loading.as_ref()
                    == Some(&(provider, base_url.clone()))
                {
                    self.fork.lyrics_ai.models_loading = None;
                }
                match result {
                    Ok(models) => {
                        self.fork.lyrics_ai.models_error = None;
                        self.fork.lyrics_ai.models = Some(crate::fork::lyrics_ai::ModelList {
                            provider,
                            base_url,
                            models,
                        });
                    }
                    Err(error) => {
                        self.fork.lyrics_ai.models_error = Some((provider, base_url, error));
                    }
                }
            }
        }
    }

    /// Asks for the genres of the playing song's artists that are not
    /// known yet, for the player bar's genre line.
    pub(super) fn request_playing_genres(&mut self) {
        if !self.settings.fork.player_bar_genres || !self.network_reachable {
            return;
        }
        let Some(now) = self.now_playing() else {
            return;
        };
        if now.is_episode {
            return;
        }
        for artist in &now.artists {
            let Some(id) = &artist.id else {
                continue;
            };
            let due = match self.fork.artist_genres.get(id) {
                None => true,
                Some(ArtistGenres::Failed(at)) => at.elapsed() >= GENRES_RETRY,
                Some(_) => false,
            };
            if due {
                self.fork
                    .artist_genres
                    .insert(id.clone(), ArtistGenres::Loading);
                self.backend
                    .send(Command::Fork(fork_mod::Command::ArtistGenres(
                        crate::fork::genres::Request {
                            id: id.clone(),
                            name: artist.name.clone(),
                        },
                    )));
            }
        }
    }

    /// The player bar's genre line for the playing song: up to three of its
    /// artists' genres, the first artist's first, as titles ("R&B").
    /// Empty when the setting is off or no artist has genres.
    pub fn playing_genres(&self, now: &NowPlaying) -> String {
        if !self.settings.fork.player_bar_genres || now.is_episode {
            return String::new();
        }
        let mut genres: Vec<&str> = Vec::new();
        for id in now.artists.iter().filter_map(|artist| artist.id.as_ref()) {
            if let Some(ArtistGenres::Known(known)) = self.fork.artist_genres.get(id) {
                for genre in known {
                    let genre = genre.trim();
                    if !genre.is_empty()
                        && !genres.iter().any(|seen| seen.eq_ignore_ascii_case(genre))
                    {
                        genres.push(genre);
                    }
                }
            }
        }
        genres
            .into_iter()
            .take(3)
            .map(crate::fork::genres::display)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Asks for the pronunciation and translation of the lyrics on show,
    /// unless they are here, on the way, or not wanted. The lyrics never
    /// wait for them.
    pub fn request_lyrics_extras(&mut self) {
        let Loadable::Loaded(Some(lyrics)) = &self.lyrics else {
            return;
        };
        let Some(uri) = self.lyrics_uri.clone() else {
            return;
        };
        if self.fork.lyrics_ai.uri.as_deref() != Some(uri.as_str()) {
            self.fork.lyrics_ai.uri = Some(uri.clone());
            self.fork.lyrics_ai.id = None;
            self.fork.lyrics_ai.extras = None;
            self.fork.lyrics_ai.error = None;
            self.fork.lyrics_ai.unanswered = false;
            self.fork.lyrics_ai.pending = None;
        }
        let settings = &self.settings.fork.lyrics_ai;
        if lyrics.instrumental
            || self.offline
            || self
                .fork
                .lyrics_ai
                .keys
                .is_some_and(|keys| !keys.has(settings.provider))
        {
            return;
        }
        let lines: Vec<&str> = lyrics.lines.iter().map(|line| line.text.as_str()).collect();
        let target = settings.target(self.locale);
        let Some(wanted) = crate::fork::lyrics_ai::wanted(&lines, settings, target) else {
            return;
        };
        let (title, artist) = match self.now_playing().filter(|now| now.uri == uri) {
            Some(now) => (
                now.title,
                now.artists
                    .first()
                    .map(|artist| artist.name.clone())
                    .unwrap_or_default(),
            ),
            None => Default::default(),
        };
        let request = crate::fork::lyrics_ai::Request {
            uri,
            title,
            artist,
            lines: lines.iter().map(|line| line.to_string()).collect(),
            target: target.tag,
            wanted,
            provider: settings.provider,
            model: settings.model().to_string(),
            base_url: settings.base_url().to_string(),
            fallbacks: self.lyrics_ai_fallbacks(settings),
        };
        let id = request.id();
        if self.fork.lyrics_ai.id.as_deref() == Some(id.as_str()) {
            return;
        }
        self.fork.lyrics_ai.id = Some(id);
        self.fork.lyrics_ai.pending = Some(wanted);
        self.backend
            .send(Command::Fork(fork_mod::Command::LyricsExtras(Box::new(
                request,
            ))));
    }

    /// What to try when the chosen model fails. With the free-models
    /// filter on, the other free models just fetched for this provider come
    /// first (a few, in list order), then the other provider.
    fn lyrics_ai_fallbacks(
        &self,
        settings: &crate::fork::lyrics_ai::AiSettings,
    ) -> Vec<crate::fork::lyrics_ai::Alternative> {
        const FREE_TRIES: usize = 4;
        if !settings.fallback {
            return Vec::new();
        }
        let mut chain = Vec::new();
        if settings.free_models_only {
            let base_url = settings.base_url();
            if let Some(list) = self
                .fork
                .lyrics_ai
                .models
                .as_ref()
                .filter(|list| list.provider == settings.provider && list.base_url == base_url)
            {
                chain.extend(
                    list.models
                        .iter()
                        .filter(|choice| choice.free && choice.id != settings.model())
                        .take(FREE_TRIES)
                        .map(|choice| crate::fork::lyrics_ai::Alternative {
                            provider: settings.provider,
                            model: choice.id.clone(),
                            base_url: base_url.to_string(),
                        }),
                );
            }
        }
        chain.push(settings.alternative());
        chain
    }

    /// Asks Unison for the playing lyrics' word timing, once per song,
    /// while the word-by-word light is on.
    pub fn request_word_timing(&mut self) {
        if !self.settings.fork.lyrics_page.word_highlight || self.offline {
            return;
        }
        let Loadable::Loaded(Some(lyrics)) = &self.lyrics else {
            return;
        };
        let Some(uri) = self.lyrics_uri.clone() else {
            return;
        };
        if !lyrics.synced || self.fork.word_timing.uri.as_deref() == Some(uri.as_str()) {
            return;
        }
        let Some(now) = self.now_playing().filter(|now| now.uri == uri) else {
            return;
        };
        self.fork.word_timing = crate::fork::word_timing::State {
            uri: Some(uri.clone()),
            ..Default::default()
        };
        self.backend
            .send(Command::Fork(fork_mod::Command::FindWordTiming(Box::new(
                crate::fork::word_timing::Request {
                    uri,
                    title: now.title,
                    artist: now
                        .artists
                        .first()
                        .map(|artist| artist.name.clone())
                        .unwrap_or_default(),
                    album: now.album_name,
                    duration_ms: now.duration_ms,
                },
            ))));
    }

    /// Matches the timed lines to the lyrics on show, once each.
    pub fn match_word_timing(&mut self) {
        let Loadable::Loaded(Some(lyrics)) = &self.lyrics else {
            return;
        };
        let (Some(uri), Some(Some(timed))) = (&self.lyrics_uri, &self.fork.word_timing.lines)
        else {
            return;
        };
        if self.fork.word_timing.uri.as_ref() != Some(uri) {
            return;
        }
        let key = Some((uri.clone(), lyrics.lines.len()));
        if self.fork.word_timing.matched_for == key {
            return;
        }
        let shown: Vec<(Option<u32>, String)> = lyrics
            .lines
            .iter()
            .map(|line| (line.at_ms, line.text.clone()))
            .collect();
        self.fork.word_timing.matched = crate::fork::word_timing::match_lines(&shown, timed);
        self.fork.word_timing.matched_for = key;
        log::debug!(
            "word timing: {} of {} lines timed word by word",
            self.fork
                .word_timing
                .matched
                .iter()
                .filter(|line| line.is_some())
                .count(),
            shown.len()
        );
    }

    fn receive_lyrics_extras(
        &mut self,
        id: &str,
        result: Result<crate::fork::lyrics_ai::Extras, crate::fork::lyrics_ai::Error>,
    ) {
        if self.fork.lyrics_ai.id.as_deref() != Some(id) {
            return;
        }
        self.fork.lyrics_ai.pending = None;
        match result {
            Ok(extras) => {
                self.fork.lyrics_ai.extras = Some(extras);
                self.fork.lyrics_ai.error = None;
                // The lines grew; put the sung one back where it sits.
                self.lyrics_line_shown = None;
            }
            Err(crate::fork::lyrics_ai::Error::NoKey) => self.fork.lyrics_ai.unanswered = true,
            Err(crate::fork::lyrics_ai::Error::Failed(error)) => {
                self.fork.lyrics_ai.error = Some(error);
            }
        }
    }
}
