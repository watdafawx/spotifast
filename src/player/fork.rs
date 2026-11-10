//! Fork: the waveform seek bar's feed from the player (FORK.md). The
//! player's events say which song plays; the song's cached file is decoded
//! here into the picture.

use super::*;

/// What the player's events tell the waveform seek bar.
pub(super) struct SeekWaveFeed {
    wave: Arc<crate::fork::seekwave::SeekWave>,
    eq: crate::eq::SharedEq,
    /// The song the player last changed to, for when the picture is turned
    /// on partway through it.
    playing: Arc<Mutex<Option<AudioItem>>>,
}

impl SeekWaveFeed {
    /// Feeds `wave`, and completes the playing song's picture whenever it
    /// is turned on, until the player's events end.
    pub(super) fn start(
        wave: Arc<crate::fork::seekwave::SeekWave>,
        eq: crate::eq::SharedEq,
    ) -> Self {
        let feed = Self {
            wave,
            eq,
            playing: Arc::default(),
        };
        let (wave, eq, playing) = (
            Arc::clone(&feed.wave),
            Arc::clone(&feed.eq),
            Arc::downgrade(&feed.playing),
        );
        tokio::spawn(async move {
            loop {
                wave.turned_on().await;
                let Some(playing) = playing.upgrade() else {
                    return;
                };
                let item = playing.lock().unwrap_or_else(|p| p.into_inner()).clone();
                if let Some(item) = item {
                    start_seek_wave(&wave, &eq, &item);
                }
            }
        });
        feed
    }

    pub(super) fn handle_player_event(&self, event: &PlayerEvent) {
        if let PlayerEvent::TrackChanged { audio_item } = event {
            *self.playing.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(audio_item.as_ref().clone());
        }
        if !self.wave.enabled() {
            return;
        }
        match event {
            PlayerEvent::TrackChanged { audio_item } => {
                start_seek_wave(&self.wave, &self.eq, audio_item);
            }
            PlayerEvent::Loading {
                track_id,
                position_ms,
                ..
            }
            | PlayerEvent::Playing {
                track_id,
                position_ms,
                ..
            }
            | PlayerEvent::Seeked {
                track_id,
                position_ms,
                ..
            }
            | PlayerEvent::PositionCorrection {
                track_id,
                position_ms,
                ..
            } => self.wave.anchor(&track_id.to_uri(), *position_ms),
            _ => {}
        }
    }
}

/// Follows the song in `item` and starts completing its picture, once.
fn start_seek_wave(
    wave: &Arc<crate::fork::seekwave::SeekWave>,
    eq: &crate::eq::SharedEq,
    item: &AudioItem,
) {
    let generation = wave.follow(&item.uri, item.duration_ms, 0);
    if wave.wants(generation) && wave.claim(generation) {
        tokio::spawn(complete_seek_wave(
            Arc::clone(wave),
            generation,
            item.clone(),
            Arc::clone(eq),
        ));
    }
}

/// How long a song plays before its cached file is decoded, so skipping
/// through songs never asks Spotify for extra keys.
const SEEK_WAVE_SETTLE: Duration = Duration::from_secs(5);
/// How often to look for the song's file in librespot's audio cache.
const SEEK_WAVE_POLL: Duration = Duration::from_secs(1);
/// How long a new song waits before asking for its audio analysis, so a
/// picture kept on disk wins and a quick skip asks nothing.
const SEEK_WAVE_ANALYSIS_DELAY: Duration = Duration::from_millis(400);

/// Completes the waveform of the song in `item`: at once from Spotify's
/// audio analysis when spclient has one, else from librespot's audio cache
/// once its whole file is there (decrypted and decoded on a blocking
/// thread). Without either, the picture fills in from playback alone.
async fn complete_seek_wave(
    wave: Arc<crate::fork::seekwave::SeekWave>,
    generation: u64,
    item: AudioItem,
    eq: crate::eq::SharedEq,
) {
    let Ok(track_id) = SpotifyId::try_from(&item.track_id) else {
        return;
    };
    tokio::time::sleep(SEEK_WAVE_ANALYSIS_DELAY).await;
    if !wave.wants(generation) {
        return;
    }
    if let Some(session) = wave.session().filter(|session| !session.is_invalid())
        && let Some(values) = seek_wave_analysis(&session, &track_id).await
    {
        wave.install_analysis(generation, values);
        return;
    }
    let started = Instant::now();
    let give_up =
        started + Duration::from_millis(u64::from(item.duration_ms)) + Duration::from_secs(60);
    let mut last_size = None;
    let (session, format, file_id, path) = loop {
        if !wave.wants(generation) || Instant::now() > give_up {
            return;
        }
        let Some(session) = wave.session().filter(|session| !session.is_invalid()) else {
            return;
        };
        let Some(cache) = session.cache() else {
            return;
        };
        let mut found = None;
        for (&format, &file_id) in item.files.iter() {
            let Some(path) = cache.file_path(file_id) else {
                // No audio cache: playback alone fills the picture.
                return;
            };
            if let Ok(meta) = std::fs::metadata(&path) {
                found = Some((format, file_id, path, meta.len()));
                break;
            }
        }
        if let Some((format, file_id, path, size)) = found {
            // librespot writes the file in place once it has it all: the
            // same size twice running means the write is done.
            let settled = last_size == Some(size) && started.elapsed() >= SEEK_WAVE_SETTLE;
            if settled && !session.audio_key().is_backing_off() {
                break (session, format, file_id, path);
            }
            last_size = Some(size);
        }
        tokio::time::sleep(SEEK_WAVE_POLL).await;
    };
    let key = match session.audio_key().request(track_id, file_id).await {
        Ok(key) => key,
        Err(error) => {
            log::debug!("waveform left to playback, no key: {error}");
            return;
        }
    };
    drop(session);
    let duration_ms = item.duration_ms;
    let decoded = tokio::task::spawn_blocking(move || {
        decode_seek_wave(&path, Some(key), format, duration_ms, eq)
    })
    .await;
    match decoded {
        Ok(Some(values)) => wave.install(generation, values, true),
        Ok(None) => log::debug!("waveform left to playback, file unreadable"),
        Err(error) => log::debug!("waveform decode failed: {error}"),
    }
}

/// The song's waveform from Spotify's audio analysis, the request Spotify's
/// own client makes through spclient; `None` when it has none (404) or the
/// request fails, and the picture is worked out from the sound instead.
async fn seek_wave_analysis(
    session: &librespot_core::Session,
    track_id: &SpotifyId,
) -> Option<Vec<u8>> {
    let endpoint = format!(
        "/audio-attributes/v1/audio-analysis/{}",
        track_id.to_base62()
    );
    match session
        .spclient()
        .request_as_json(&reqwest::Method::GET, &endpoint, None, None)
        .await
    {
        Ok(bytes) => {
            let values = crate::fork::seekwave::from_analysis(&bytes);
            log::info!(
                "waveform analysis {}: {}",
                track_id.to_base62(),
                if values.is_some() {
                    "found"
                } else {
                    "unreadable"
                }
            );
            values
        }
        Err(error) => {
            log::info!(
                "waveform analysis {}: not had ({error})",
                track_id.to_base62()
            );
            None
        }
    }
}

/// The levels of the cached, encrypted file at `path`, through the same
/// equalizer the player uses.
fn decode_seek_wave(
    path: &std::path::Path,
    key: Option<librespot_core::audio_key::AudioKey>,
    format: librespot_metadata::audio::AudioFileFormat,
    duration_ms: u32,
    eq: crate::eq::SharedEq,
) -> Option<Vec<u8>> {
    use librespot_metadata::audio::AudioFiles;
    use librespot_playback::decoder::{AudioDecoder, AudioPacket, SymphoniaDecoder};
    use symphonia::core::probe::Hint;
    /// The header Spotify puts in front of its Ogg files.
    const SPOTIFY_OGG_HEADER_END: u64 = 0xa7;

    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let offset = if AudioFiles::is_ogg_vorbis(format) {
        SPOTIFY_OGG_HEADER_END
    } else {
        0
    };
    let source = crate::fork::seekwave::Offset::new(
        librespot_audio::AudioDecrypt::new(key, file),
        offset,
        len,
    )
    .ok()?;
    let mut hint = Hint::new();
    if let Some(mime) = AudioFiles::mime_type(format) {
        hint.mime_type(mime);
    }
    let mut decoder = SymphoniaDecoder::new(source, hint).ok()?;
    let mut eq = crate::eq::Processor::new(eq);
    let expected = u64::from(duration_ms) * u64::from(librespot_playback::SAMPLE_RATE) / 1_000;
    let mut builder = crate::fork::seekwave::Builder::new(expected);
    let mut frame = 0;
    while let Ok(Some((_, packet))) = decoder.next_packet() {
        if let AudioPacket::Samples(mut samples) = packet {
            eq.process(&mut samples);
            frame = builder.add(frame, &samples, 1.0, true);
        }
    }
    // A file that stops short decodes only part of the song; leave the
    // rest to playback rather than stretch what was read over it.
    (frame >= expected * 9 / 10)
        .then(|| builder.finish())
        .flatten()
}

#[cfg(test)]
mod tests {
    /// Decodes an Ogg Vorbis file behind Spotify's 0xa7-byte header, as
    /// the waveform seek bar reads a cached song, and reports the time it
    /// took. Run with `SPOTIFAST_WAVE_OGG=<file.ogg>` and `--ignored`.
    #[test]
    #[ignore = "needs an Ogg Vorbis file"]
    fn a_cached_song_decodes_to_a_whole_waveform() {
        let ogg = std::env::var("SPOTIFAST_WAVE_OGG").expect("SPOTIFAST_WAVE_OGG");
        let mut bytes = vec![0; 0xa7];
        bytes.extend(std::fs::read(&ogg).unwrap());
        let path = std::env::temp_dir().join(format!("spotifast-wave-{}.ogg", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let duration_ms: u32 = std::env::var("SPOTIFAST_WAVE_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(240_000);
        let started = std::time::Instant::now();
        let values = super::decode_seek_wave(
            &path,
            None,
            librespot_metadata::audio::AudioFileFormat::OGG_VORBIS_320,
            duration_ms,
            crate::eq::shared(),
        );
        let took = started.elapsed();
        let _ = std::fs::remove_file(&path);
        let values = values.expect("decoded");
        assert_eq!(values.len(), crate::fork::seekwave::BUCKETS);
        assert!(values.iter().any(|&value| value > 0));
        println!("decoded {duration_ms} ms of sound in {took:?}");
    }
}
