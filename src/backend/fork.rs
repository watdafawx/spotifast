//! Fork: the fork's part of the backend worker. `backend.rs` reaches it
//! through `Command::Fork`, `Event::Fork` and the worker's `fork` field.

use super::*;

/// The fork's part of [`Worker`].
#[derive(Default)]
pub(super) struct Worker {
    /// The AI keys, once read from the credential store.
    ai_keys: Option<crate::fork::lyrics_ai::Keys>,
    /// Whether the AI keys are being read.
    ai_keys_loading: bool,
    /// Extra-line requests waiting for the AI keys.
    ai_waiting: Vec<crate::fork::lyrics_ai::Request>,
    /// A model-list request waiting for the AI keys.
    ai_models_waiting: Option<(crate::fork::lyrics_ai::Provider, String)>,
    /// Keys typed before the saved ones were read, applied after.
    ai_key_edits: Vec<(
        crate::fork::lyrics_ai::KeyFor,
        crate::fork::lyrics_ai::SecretKey,
    )>,
    /// Genre lookups waiting for the saved keys, for the Last.fm one.
    genres_waiting: Vec<crate::fork::genres::Request>,
    /// Keeps MusicBrainz requests a second apart.
    genre_gate: crate::fork::genres::Gate,
}

impl super::Worker {
    /// `Command::Fork`.
    pub(super) fn fork_command(&mut self, command: crate::fork::Command) {
        use crate::fork::Command as C;
        match command {
            C::LyricsExtras(request) => self.fetch_lyrics_extras(*request),
            C::LoadAiKeys => self.load_ai_keys(),
            C::ListAiModels { provider, base_url } => self.list_ai_models(provider, base_url),
            C::FindLyricsVideo(request) => self.find_lyrics_video(*request, None),
            C::FindWordTiming(request) => self.find_word_timing(*request),
            C::ChooseLyricsVideo(request, choice) => {
                self.find_lyrics_video(*request, Some(choice));
            }
            C::SetAiKey { provider, key } => {
                self.set_ai_key(crate::fork::lyrics_ai::KeyFor::Ai(provider), key);
            }
            C::SetLastFmKey(key) => self.set_ai_key(crate::fork::lyrics_ai::KeyFor::LastFm, key),
            C::ArtistGenres(request) => self.fetch_artist_genres(request),
            C::AiKeysLoaded { lease, result } => self.on_ai_keys_loaded(lease, result),
            #[cfg(feature = "mods")]
            C::Mods(request) => {
                let (engine, api) = (self.engine.clone(), self.api.clone());
                tokio::spawn(crate::fork::mods::proxy::serve(*request, engine, api));
            }
        }
    }

    fn fetch_lyrics_extras(&mut self, request: crate::fork::lyrics_ai::Request) {
        let Some(keys) = self.fork.ai_keys.clone() else {
            self.fork.ai_waiting.push(request);
            self.load_ai_keys();
            return;
        };
        let http = self.http.client();
        let events = self.events.clone();
        let waker = self.waker.clone();
        let cache_dir = crate::fork::lyrics_ai::cache_dir(&self.dirs.lyrics_cache_dir());
        tokio::spawn(async move {
            let id = request.id();
            let result = match http {
                Ok(http) => crate::fork::lyrics_ai::fetch(&http, &cache_dir, &request, &keys).await,
                Err(error) => Err(crate::fork::lyrics_ai::Error::Failed(error)),
            };
            if let Err(crate::fork::lyrics_ai::Error::Failed(error)) = &result {
                log::warn!("lyrics extras unavailable: {error}");
            }
            let _ = events.send(Event::Fork(crate::fork::Event::LyricsExtras { id, result }));
            waker.wake();
        });
    }

    /// Looks up an artist's genres, once the saved keys say whether Last.fm
    /// can fill in for MusicBrainz.
    fn fetch_artist_genres(&mut self, request: crate::fork::genres::Request) {
        let Some(keys) = &self.fork.ai_keys else {
            self.fork.genres_waiting.push(request);
            self.load_ai_keys();
            return;
        };
        let lastfm = Some(keys.lastfm.trim().to_string()).filter(|key| !key.is_empty());
        let http = self.http.client();
        let events = self.events.clone();
        let waker = self.waker.clone();
        let gate = self.fork.genre_gate.clone();
        let cache_dir = self.dirs.genre_cache_dir();
        tokio::spawn(async move {
            let result = match http {
                Ok(http) => {
                    crate::fork::genres::fetch(
                        &http,
                        &gate,
                        &cache_dir,
                        &request,
                        lastfm.as_deref(),
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            if let Err(error) = &result {
                log::warn!("genres for artist {} unavailable: {error}", request.id);
            }
            let _ = events.send(Event::Fork(crate::fork::Event::ArtistGenres {
                id: request.id,
                result,
            }));
            waker.wake();
        });
    }

    fn find_word_timing(&self, request: crate::fork::word_timing::Request) {
        let http = self.http.client();
        let events = self.events.clone();
        let waker = self.waker.clone();
        let cache_dir = crate::fork::word_timing::cache_dir(&self.dirs.lyrics_cache_dir());
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let result = async {
                let http = http?;
                // The ISRC finds the exact recording; without it, the name.
                let isrc = track_isrc(engine, &request.uri).await.ok();
                crate::fork::word_timing::fetch(&http, &cache_dir, &request, isrc.as_deref()).await
            }
            .await;
            if let Err(error) = &result {
                log::debug!("word timing unavailable: {error}");
            }
            let _ = events.send(Event::Fork(crate::fork::Event::WordTiming {
                uri: request.uri,
                result,
            }));
            waker.wake();
        });
    }

    fn find_lyrics_video(
        &self,
        request: crate::fork::lyrics_video::Request,
        choice: Option<crate::fork::lyrics_video::Choice>,
    ) {
        let events = self.events.clone();
        let waker = self.waker.clone();
        let cache_dir = crate::fork::lyrics_video::cache_dir(&self.dirs.lyrics_cache_dir());
        tokio::task::spawn_blocking(move || {
            let result = match &choice {
                Some(choice) => crate::fork::lyrics_video::choose(&cache_dir, &request, choice),
                None => crate::fork::lyrics_video::find(&cache_dir, &request),
            };
            if let Err(error) = &result {
                log::debug!("lyrics video unavailable: {error}");
            }
            let _ = events.send(Event::Fork(crate::fork::Event::LyricsVideo {
                uri: request.uri,
                result,
            }));
            waker.wake();
        });
    }

    fn list_ai_models(&mut self, provider: crate::fork::lyrics_ai::Provider, base_url: String) {
        let Some(keys) = self.fork.ai_keys.clone() else {
            self.fork.ai_models_waiting = Some((provider, base_url));
            self.load_ai_keys();
            return;
        };
        let http = self.http.client();
        let events = self.events.clone();
        let waker = self.waker.clone();
        tokio::spawn(async move {
            let result = match http {
                Ok(http) => {
                    crate::fork::lyrics_ai::list_models(&http, provider, &base_url, &keys).await
                }
                Err(error) => Err(error),
            };
            let _ = events.send(Event::Fork(crate::fork::Event::AiModels {
                provider,
                base_url,
                result,
            }));
            waker.wake();
        });
    }

    fn load_ai_keys(&mut self) {
        if let Some(keys) = &self.fork.ai_keys {
            self.emit(Event::Fork(crate::fork::Event::AiKeys(Ok(keys.present()))));
            return;
        }
        if std::mem::replace(&mut self.fork.ai_keys_loading, true) {
            return;
        }
        let lease = self.credentials.lease(CredentialSlot::AiKeys);
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let result = lease.load().await;
            let _ = commands.send(Command::Fork(crate::fork::Command::AiKeysLoaded {
                lease,
                result,
            }));
        });
    }

    fn on_ai_keys_loaded(
        &mut self,
        lease: CredentialLease,
        result: Result<crate::credentials::Loaded, crate::credentials::Error>,
    ) {
        if !lease.current() {
            return;
        }
        self.fork.ai_keys_loading = false;
        let keys = match result {
            Ok(loaded) => match loaded.grant {
                Some(StoredGrant::AiKeys(keys)) => keys,
                _ => crate::fork::lyrics_ai::Keys::default(),
            },
            Err(error) => {
                self.emit(Event::Fork(crate::fork::Event::AiKeys(Err(
                    error.to_string()
                ))));
                crate::fork::lyrics_ai::Keys::default()
            }
        };
        self.emit(Event::Fork(crate::fork::Event::AiKeys(Ok(keys.present()))));
        self.fork.ai_keys = Some(keys);
        for (which, key) in std::mem::take(&mut self.fork.ai_key_edits) {
            self.set_ai_key(which, key);
        }
        for request in std::mem::take(&mut self.fork.genres_waiting) {
            self.fetch_artist_genres(request);
        }
        for request in std::mem::take(&mut self.fork.ai_waiting) {
            self.fetch_lyrics_extras(request);
        }
        if let Some((provider, base_url)) = self.fork.ai_models_waiting.take() {
            self.list_ai_models(provider, base_url);
        }
    }

    fn set_ai_key(
        &mut self,
        which: crate::fork::lyrics_ai::KeyFor,
        key: crate::fork::lyrics_ai::SecretKey,
    ) {
        // The other provider's saved key must survive, so read the saved
        // keys before changing one.
        let Some(mut keys) = self.fork.ai_keys.clone() else {
            self.fork.ai_key_edits.push((which, key));
            self.load_ai_keys();
            return;
        };
        keys.set_for(which, key.0.trim().to_string());
        self.emit(Event::Fork(crate::fork::Event::AiKeys(Ok(keys.present()))));
        let operation = if keys.is_empty() {
            if let Err(error) = self.credentials.revoke(CredentialSlot::AiKeys) {
                self.emit(Event::Fork(crate::fork::Event::AiKeys(Err(
                    error.to_string()
                ))));
            }
            self.credentials.lease(CredentialSlot::AiKeys).delete()
        } else {
            self.credentials.invalidate(CredentialSlot::AiKeys);
            self.credentials
                .lease(CredentialSlot::AiKeys)
                .save(StoredGrant::AiKeys(keys.clone()))
        };
        self.fork.ai_keys = Some(keys);
        let events = self.events.clone();
        let waker = self.waker.clone();
        tokio::spawn(async move {
            match operation.await {
                Ok(()) | Err(crate::credentials::Error::Stale) => {}
                Err(error) => {
                    // Still usable for this session.
                    let _ = events.send(Event::Fork(crate::fork::Event::AiKeys(Err(
                        error.to_string()
                    ))));
                    waker.wake();
                }
            }
        });
    }
}

/// A track's ISRC from Spotify's own metadata.
async fn track_isrc(engine: Option<Arc<Engine>>, uri: &str) -> Result<String, String> {
    use librespot_metadata::{Metadata as _, Track};
    let id = librespot_core::SpotifyUri::from_uri(uri).map_err(|error| error.to_string())?;
    let engine = engine.ok_or("local playback is not signed in")?;
    let track = Track::get(engine.session(), &id)
        .await
        .map_err(|error| error.to_string())?;
    track
        .external_ids
        .iter()
        .find(|external| external.external_type.eq_ignore_ascii_case("isrc"))
        .map(|external| external.id.clone())
        .ok_or_else(|| "Spotify has no ISRC for this track".to_string())
}
