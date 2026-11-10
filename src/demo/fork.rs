//! Fork: the demo's fork fixtures and screenshot flags (FORK.md).

use super::*;

/// Hook at the end of `populate`'s artist page: the player bar's genre
/// line under the playing artist.
pub(super) fn populate(app: &mut App) {
    for index in 0..ARTISTS.len() {
        app.fork.artist_genres.insert(
            format!("art{index}"),
            crate::fork::ArtistGenres::Known(artist(index).genres),
        );
    }
}

/// A song's loudness for the waveform seek bar, the same every time: a
/// quiet intro, verses and louder choruses with a breakdown between, and a
/// fade at the end, with a beat's ripple through it all.
pub fn demo_seek_levels() -> Vec<u8> {
    let steps = crate::fork::seekwave::BUCKETS;
    (0..steps)
        .map(|step| {
            let at = step as f32 / steps as f32;
            let section = match at {
                a if a < 0.08 => 0.18 + a * 4.0,
                a if a < 0.30 => 0.55,
                a if a < 0.45 => 0.9,
                a if a < 0.52 => 0.3,
                a if a < 0.70 => 0.6,
                a if a < 0.90 => 0.95,
                a => 0.95 * (1.0 - (a - 0.9) * 9.0),
            };
            let ripple = 0.82 + 0.18 * ((step * 7919 % 97) as f32 / 97.0);
            let beat = if step % 4 == 0 { 1.0 } else { 0.86 };
            let rms: f32 = (section * ripple * beat).clamp(0.0, 1.0);
            // Stored as the square root of the level, as the cache keeps it.
            (rms.sqrt() * 255.0).round() as u8
        })
        .collect()
}

/// Hook in `apply_flags`: the fork's surfaces. True when `surface` was one.
#[cfg(feature = "demo")]
pub(super) fn apply_flag(app: &mut App, surface: &str) -> bool {
    match surface {
        // The waveform seek bar, whole for a song heard before, and part
        // way through filling in on a first play.
        "seek-waveform" | "seek-waveform-filling" => {
            app.settings.fork.seek_waveform = true;
            play_here(app);
            app.local.playback = crate::player::Playback::Paused;
            if let Some(track) = &app.local.track {
                let duration = track.duration_ms;
                app.local.position_ms = duration * 38 / 100;
                let levels = demo_seek_levels();
                let known = if surface == "seek-waveform" {
                    levels.len()
                } else {
                    levels.len() * 45 / 100
                };
                app.winamp.tap.seek_wave.demo(&track.uri, &levels, known);
            }
        }
        // Settings filtered down to the Genres section.
        "settings-genres" => {
            app.winamp.demo_settings_filter = Some("Genres".into());
        }
        _ => return false,
    }
    true
}
