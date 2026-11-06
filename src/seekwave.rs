//! The waveform seek bar's picture of a song: how loud it is across its
//! whole length, in [`BUCKETS`] steps.
//!
//! Spotify's audio analysis is closed to new apps, so the picture is worked
//! out here from the sound itself. It fills in from the samples the player
//! decodes as the song plays (post-equalizer, pre-volume, like every
//! visualiser), and once librespot has the song's whole file in its audio
//! cache the file is decoded on a background thread to complete it at once
//! (see `player::complete_seek_wave`). A complete picture is kept on disk,
//! about a kilobyte per song, so a replay draws it straight away.
//!
//! Nothing here runs while the setting is off: pushes return at once and no
//! file is read or decoded.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// How many steps a song's picture has, whatever its length.
pub const BUCKETS: usize = 1000;
/// The share of steps heard during play that counts as the whole song, so
/// a few steps lost to a seek or a track change don't keep it from the
/// disk. The gaps take their neighbours' values.
const COMPLETE_ENOUGH: usize = BUCKETS * 99 / 100;
/// Frames a second of librespot's decoded sound.
const FRAME_RATE: u64 = 44_100;
/// The stereo channels librespot decodes to.
const CHANNELS: usize = 2;
/// The cache file: magic, version, step count, then a byte a step.
const MAGIC: &[u8; 4] = b"SFWV";
const VERSION: u8 = 1;
const HEADER: usize = 4 + 1 + 2;
/// How far below the song's loudest step the quietest bar still shows, in
/// decibels; anything quieter draws as the flat floor.
const RANGE_DB: f32 = 20.0;
/// The shortest bar, as a share of the full height, so silence still shows
/// where the song is.
pub const FLOOR: f32 = 0.08;

/// One song's loudness: each step's RMS level, companded into a byte (the
/// square root of the level, so quiet passages keep detail), and whether
/// that step has been heard or decoded yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Levels {
    pub values: Vec<u8>,
    pub filled: Vec<bool>,
}

impl Levels {
    fn complete(values: Vec<u8>) -> Self {
        let filled = vec![true; values.len()];
        Self { values, filled }
    }

    /// Whether every step is known.
    pub fn is_complete(&self) -> bool {
        self.filled.iter().all(|&filled| filled)
    }
}

/// Adds up the sound a step at a time.
#[derive(Clone, Debug)]
pub struct Builder {
    total_frames: u64,
    power: Vec<f64>,
    counts: Vec<u32>,
}

impl Builder {
    /// For a song of `total_frames` frames at 44.1 kHz.
    pub fn new(total_frames: u64) -> Self {
        Self {
            total_frames: total_frames.max(1),
            power: vec![0.0; BUCKETS],
            counts: vec![0; BUCKETS],
        }
    }

    fn for_duration(duration_ms: u32) -> Self {
        Self::new(u64::from(duration_ms) * FRAME_RATE / 1_000)
    }

    /// Adds interleaved stereo `samples` that start `start` frames into the
    /// song, each scaled by `gain`. Frames past the song's end go to its
    /// last step when `keep_tail`, and are dropped otherwise: during play
    /// they are the next song's, arriving before the player says so.
    pub fn add(&mut self, start: u64, samples: &[f64], gain: f32, keep_tail: bool) -> u64 {
        let gain = f64::from(gain);
        let (frames, _) = samples.as_chunks::<CHANNELS>();
        for (frame, [left, right]) in (start..).zip(frames) {
            if frame >= self.total_frames && !keep_tail {
                break;
            }
            let bucket = ((frame * BUCKETS as u64 / self.total_frames) as usize).min(BUCKETS - 1);
            let (left, right) = (left * gain, right * gain);
            self.power[bucket] += (left * left + right * right) / 2.0;
            self.counts[bucket] += 1;
        }
        start + (samples.len() / CHANNELS) as u64
    }

    fn filled(&self) -> usize {
        self.counts.iter().filter(|&&count| count > 0).count()
    }

    /// The levels heard so far.
    pub fn levels(&self) -> Levels {
        let values = self
            .power
            .iter()
            .zip(&self.counts)
            .map(|(&power, &count)| compand(power, count))
            .collect();
        let filled = self.counts.iter().map(|&count| count > 0).collect();
        Levels { values, filled }
    }

    /// Every step's level, a step never reached taking its nearest
    /// neighbour's. `None` when nothing was heard at all.
    pub fn finish(&self) -> Option<Vec<u8>> {
        let levels = self.levels();
        let known: Vec<usize> = (0..BUCKETS).filter(|&i| levels.filled[i]).collect();
        if known.is_empty() {
            return None;
        }
        let mut values = levels.values;
        for index in 0..BUCKETS {
            if !levels.filled[index] {
                let nearest = known
                    .iter()
                    .min_by_key(|&&known| known.abs_diff(index))
                    .copied()
                    .unwrap_or(index);
                values[index] = values[nearest];
            }
        }
        Some(values)
    }
}

fn compand(power: f64, count: u32) -> u8 {
    if count == 0 {
        return 0;
    }
    let rms = (power / f64::from(count)).sqrt().min(1.0);
    (rms.sqrt() * 255.0).round() as u8
}

fn expand(value: u8) -> f32 {
    let root = f32::from(value) / 255.0;
    root * root
}

/// `samples` (interleaved stereo, `total_frames` long) reduced to
/// [`BUCKETS`] levels.
pub fn reduce(samples: &[f64], total_frames: u64) -> Vec<u8> {
    let mut builder = Builder::new(total_frames);
    builder.add(0, samples, 1.0, true);
    builder.finish().unwrap_or_else(|| vec![0; BUCKETS])
}

/// The heights of `bars` bars across the song, from [`FLOOR`] to 1, with
/// the loudest step at full height; `None` where nothing is known yet.
pub fn bar_heights(levels: &Levels, bars: usize) -> Vec<Option<f32>> {
    let steps = levels.values.len();
    if bars == 0 || steps == 0 {
        return Vec::new();
    }
    let loudest = levels
        .values
        .iter()
        .zip(&levels.filled)
        .filter(|(_, filled)| **filled)
        .map(|(&value, _)| expand(value))
        .fold(0.0f32, f32::max);
    (0..bars)
        .map(|bar| {
            let first = bar * steps / bars;
            let last = ((bar + 1) * steps / bars).max(first + 1).min(steps);
            let level = (first..last)
                .filter(|&step| levels.filled[step])
                .map(|step| expand(levels.values[step]))
                .reduce(f32::max)?;
            if loudest <= f32::EPSILON || level <= f32::EPSILON {
                return Some(FLOOR);
            }
            let below = 20.0 * (level / loudest).log10();
            Some((1.0 + below / RANGE_DB).clamp(FLOOR, 1.0))
        })
        .collect()
}

/// The cache file's bytes for complete `values`.
pub fn encode(values: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + values.len());
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    bytes.extend_from_slice(&(values.len() as u16).to_le_bytes());
    bytes.extend_from_slice(values);
    bytes
}

/// The levels in a cache file, or `None` for a file this version cannot
/// read: another format, a later version or a short write.
pub fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let (header, values) = bytes.split_at_checked(HEADER)?;
    if &header[..4] != MAGIC || header[4] != VERSION {
        return None;
    }
    let count = usize::from(u16::from_le_bytes([header[5], header[6]]));
    (count == BUCKETS && values.len() == count).then(|| values.to_vec())
}

/// The cache file for the song at `uri`, such as `spotify:track:…`.
fn cache_file(dir: &Path, uri: &str) -> Option<PathBuf> {
    let name: String = uri
        .strip_prefix("spotify:")?
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    (!name.is_empty() && !uri.starts_with("spotify:local:")).then(|| dir.join(name + ".wave"))
}

fn load(dir: &Path, uri: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(cache_file(dir, uri)?)
        .ok()?
        .take((HEADER + BUCKETS + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    decode(&bytes)
}

/// Writes beside the file, then renames over it, so a reader never sees
/// half a file.
fn save(dir: &Path, uri: &str, values: &[u8]) {
    let Some(path) = cache_file(dir, uri) else {
        return;
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let partial = path.with_extension("wave.part");
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(&encode(values))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&partial, &path)
    };
    if let Err(error) = write() {
        log::debug!("waveform not cached: {error}");
    }
}

/// The song being pictured, shared by the player, which fills it in, and
/// the player bar, which draws it.
#[derive(Default)]
pub struct SeekWave {
    enabled: AtomicBool,
    dir: Mutex<Option<PathBuf>>,
    inner: Mutex<Inner>,
    /// The session the cached file's key is asked of, set while an engine
    /// is connected.
    session: Mutex<Option<librespot_core::Session>>,
}

#[derive(Default)]
struct Inner {
    uri: String,
    /// Bumped for each new song, so work for an earlier one is dropped.
    generation: u64,
    /// Bumped whenever the picture changes, so the bar copies it only then.
    version: u64,
    /// The next frame the player hands over, counted from the song's start.
    frame: u64,
    builder: Option<Builder>,
    complete: Option<Vec<u8>>,
}

impl std::fmt::Debug for SeekWave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SeekWave")
    }
}

/// What the player bar last copied out of [`SeekWave`].
#[derive(Clone, Debug, Default)]
pub struct View {
    pub uri: String,
    version: u64,
    pub levels: Option<Levels>,
}

impl SeekWave {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where complete pictures are kept.
    pub fn set_dir(&self, dir: PathBuf) {
        *self.dir.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir);
    }

    fn dir(&self) -> Option<PathBuf> {
        self.dir
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Turns the work on or off. Off forgets the song, freeing its picture.
    pub fn set_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::Relaxed) != enabled && !enabled {
            let mut inner = self.inner();
            let generation = inner.generation + 1;
            *inner = Inner {
                generation,
                ..Inner::default()
            };
        }
    }

    pub fn set_session(&self, session: Option<librespot_core::Session>) {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = session;
    }

    /// Lets go of the session with `session_id` when it is the one held.
    pub fn forget_session(&self, session_id: &str) {
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        if session
            .as_ref()
            .is_some_and(|held| held.session_id() == session_id)
        {
            *session = None;
        }
    }

    pub(crate) fn session(&self) -> Option<librespot_core::Session> {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Pictures the song at `uri` from now on, `duration_ms` long, and
    /// returns its generation. A new song starts empty, its sound counted
    /// from `position_ms`, and is looked up on disk on a thread of its own;
    /// the same song carries on.
    pub fn follow(self: &Arc<Self>, uri: &str, duration_ms: u32, position_ms: u32) -> u64 {
        let mut inner = self.inner();
        if !self.enabled() || inner.uri == uri {
            return inner.generation;
        }
        inner.uri = uri.to_string();
        inner.generation += 1;
        inner.version += 1;
        inner.frame = u64::from(position_ms) * FRAME_RATE / 1_000;
        inner.complete = None;
        inner.builder = (duration_ms > 0).then(|| Builder::for_duration(duration_ms));
        let generation = inner.generation;
        drop(inner);
        if let Some(dir) = self.dir() {
            let wave = Arc::clone(self);
            let uri = uri.to_string();
            let spawned = std::thread::Builder::new()
                .name("seek-wave-load".into())
                .spawn(move || {
                    if let Some(values) = load(&dir, &uri) {
                        wave.install(generation, values, false);
                    }
                });
            if let Err(error) = spawned {
                log::debug!("waveform lookup not started: {error}");
            }
        }
        generation
    }

    /// The decoder is at `position_ms` into the song at `uri`.
    pub fn anchor(&self, uri: &str, position_ms: u32) {
        let mut inner = self.inner();
        if inner.uri == uri {
            inner.frame = u64::from(position_ms) * FRAME_RATE / 1_000;
        }
    }

    /// Whether `generation` is still the song pictured, and is not complete.
    pub fn wants(&self, generation: u64) -> bool {
        let inner = self.inner();
        self.enabled() && inner.generation == generation && inner.complete.is_none()
    }

    /// Adds the player's decoded sound, interleaved stereo, scaled by
    /// `gain`. Once nearly every step has been heard the picture is kept.
    pub fn push(self: &Arc<Self>, samples: &[f64], gain: f32) {
        if !self.enabled() {
            return;
        }
        let mut inner = self.inner();
        if inner.complete.is_some() {
            return;
        }
        let start = inner.frame;
        let Some(builder) = inner.builder.as_mut() else {
            return;
        };
        let end = builder.add(start, samples, gain, false);
        let done = (builder.filled() >= COMPLETE_ENOUGH)
            .then(|| builder.finish())
            .flatten();
        inner.frame = end;
        inner.version += 1;
        if let Some(values) = done {
            let generation = inner.generation;
            drop(inner);
            // Off the player's thread: it must not wait for the disk.
            let wave = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("seek-wave-save".into())
                .spawn(move || wave.install(generation, values, true));
            if let Err(error) = spawned {
                log::debug!("waveform not kept: {error}");
            }
        }
    }

    /// Completes the picture of `generation` with `values`, keeping them on
    /// disk when `keep`.
    pub fn install(&self, generation: u64, values: Vec<u8>, keep: bool) {
        if values.len() != BUCKETS {
            return;
        }
        let mut inner = self.inner();
        if inner.generation != generation || !self.enabled() {
            return;
        }
        let uri = inner.uri.clone();
        inner.builder = None;
        inner.complete = Some(values.clone());
        inner.version += 1;
        drop(inner);
        if keep && let Some(dir) = self.dir() {
            save(&dir, &uri, &values);
        }
    }

    /// Shows `values` for the song at `uri` with only its first `known`
    /// steps heard, for demo screenshots.
    #[cfg(feature = "demo")]
    pub fn demo(&self, uri: &str, values: &[u8], known: usize) {
        self.enabled.store(true, Ordering::Relaxed);
        let mut inner = self.inner();
        inner.uri = uri.to_string();
        inner.generation += 1;
        inner.version += 1;
        if known >= values.len() {
            inner.builder = None;
            inner.complete = Some(values.to_vec());
            return;
        }
        let mut builder = Builder::new(BUCKETS as u64);
        for (step, &value) in values.iter().enumerate().take(known) {
            let level = f64::from(expand(value));
            builder.add(step as u64, &[level, level], 1.0, false);
        }
        inner.complete = None;
        inner.builder = Some(builder);
    }

    /// Brings `view` up to date for the song at `uri`, copying only what
    /// changed since it was last brought up to date.
    pub fn refresh(&self, uri: &str, view: &mut View) {
        let inner = self.inner();
        if inner.uri != uri {
            if view.uri != uri || view.levels.is_some() {
                *view = View {
                    uri: uri.to_string(),
                    ..View::default()
                };
            }
            return;
        }
        if view.uri == uri && view.version == inner.version {
            return;
        }
        view.uri = uri.to_string();
        view.version = inner.version;
        view.levels = match (&inner.complete, &inner.builder) {
            (Some(values), _) => Some(Levels::complete(values.clone())),
            (None, Some(builder)) => Some(builder.levels()),
            (None, None) => None,
        };
    }
}

/// A file with its first `offset` bytes hidden, for the decoder: Spotify's
/// Ogg files start with a header of their own.
pub(crate) struct Offset<R> {
    inner: R,
    offset: u64,
    len: u64,
}

impl<R: Read + Seek> Offset<R> {
    pub(crate) fn new(mut inner: R, offset: u64, file_len: u64) -> std::io::Result<Self> {
        inner.seek(SeekFrom::Start(offset))?;
        Ok(Self {
            inner,
            offset,
            len: file_len.saturating_sub(offset),
        })
    }
}

impl<R: Read> Read for Offset<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Seek> Seek for Offset<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let pos = match pos {
            SeekFrom::Start(at) => SeekFrom::Start(at + self.offset),
            other => other,
        };
        let at = self.inner.seek(pos)?;
        Ok(at.saturating_sub(self.offset))
    }
}

impl<R: Read + Seek + Send + Sync> symphonia::core::io::MediaSource for Offset<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, level: f64) -> Vec<f64> {
        (0..frames)
            .flat_map(|i| {
                let s = if i % 2 == 0 { level } else { -level };
                [s, s]
            })
            .collect()
    }

    #[test]
    fn reduction_gives_each_step_its_rms_level() {
        // First half at full scale, second half at a quarter.
        let frames = BUCKETS * 50;
        let mut samples = tone(frames / 2, 1.0);
        samples.extend(tone(frames / 2, 0.25));
        let values = reduce(&samples, frames as u64);
        assert_eq!(values.len(), BUCKETS);
        assert_eq!(values[0], 255);
        assert_eq!(values[BUCKETS / 2 - 1], 255);
        // sqrt(0.25) of full scale.
        assert_eq!(values[BUCKETS / 2], 128);
        assert_eq!(values[BUCKETS - 1], 128);
    }

    #[test]
    fn reduction_of_silence_is_flat_and_safe() {
        let values = reduce(&vec![0.0; 2 * 4410], 4410);
        assert!(values.iter().all(|&v| v == 0));
        assert_eq!(reduce(&[], 0).len(), BUCKETS);
    }

    #[test]
    fn a_longer_decode_than_the_listed_length_stays_in_the_last_step() {
        let frames = BUCKETS * 10;
        let values = reduce(&tone(frames + 500, 0.5), frames as u64);
        assert_eq!(values.len(), BUCKETS);
        assert!(values.iter().all(|&v| v == values[0]));
    }

    #[test]
    fn played_sound_past_the_end_belongs_to_the_next_song() {
        let mut builder = Builder::new(BUCKETS as u64);
        let end = builder.add(BUCKETS as u64 - 1, &tone(10, 1.0), 1.0, false);
        assert_eq!(end, BUCKETS as u64 + 9);
        assert_eq!(builder.filled(), 1);
    }

    #[test]
    fn a_seek_leaves_gaps_that_finish_fills_from_neighbours() {
        let frames = (BUCKETS * 10) as u64;
        let mut builder = Builder::new(frames);
        builder.add(0, &tone(100, 1.0), 1.0, false);
        builder.add(frames - 100, &tone(100, 0.25), 1.0, false);
        let levels = builder.levels();
        assert!(levels.filled[0] && !levels.filled[500] && levels.filled[BUCKETS - 1]);
        let values = builder.finish().unwrap();
        assert_eq!(values[1], 255);
        assert_eq!(values[BUCKETS - 2], 128);
        assert!(Builder::new(frames).finish().is_none());
    }

    #[test]
    fn the_cache_format_round_trips_and_refuses_anything_else() {
        let values: Vec<u8> = (0..BUCKETS).map(|i| (i % 256) as u8).collect();
        let bytes = encode(&values);
        assert_eq!(bytes.len(), HEADER + BUCKETS);
        assert_eq!(&bytes[..5], b"SFWV\x01");
        assert_eq!(decode(&bytes), Some(values.clone()));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(&bytes[..3]), None);
        let mut later = bytes.clone();
        later[4] = 2;
        assert_eq!(decode(&later), None);
        let mut other = bytes;
        other[0] = b'X';
        assert_eq!(decode(&other), None);
    }

    #[test]
    fn cache_files_are_named_after_the_song_and_skip_local_files() {
        let dir = Path::new("waves");
        assert_eq!(
            cache_file(dir, "spotify:track:4uLU6hMCjMI75M1A2tKUQC"),
            Some(dir.join("track-4uLU6hMCjMI75M1A2tKUQC.wave"))
        );
        assert_eq!(cache_file(dir, "spotify:local:a:b:c:180"), None);
        assert_eq!(cache_file(dir, "../../etc"), None);
    }

    #[test]
    fn a_kept_picture_is_found_again_for_the_same_song() {
        let dir = std::env::temp_dir().join(format!("spotifast-wave-{}", std::process::id()));
        let uri = "spotify:track:4uLU6hMCjMI75M1A2tKUQC";
        let values = vec![42; BUCKETS];
        save(&dir, uri, &values);
        assert_eq!(load(&dir, uri), Some(values));
        assert_eq!(load(&dir, "spotify:track:other"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bars_scale_to_the_loudest_step_and_mark_unknown_steps() {
        let mut values = vec![255; BUCKETS];
        values[BUCKETS / 2..].fill(0);
        let mut filled = vec![true; BUCKETS];
        filled[BUCKETS - 1] = false;
        let levels = Levels { values, filled };
        let bars = bar_heights(&levels, 4);
        assert_eq!(bars[0], Some(1.0));
        assert_eq!(bars[2], Some(FLOOR));
        // The last bar still has known steps.
        assert_eq!(bars[3], Some(FLOOR));
        let unknown = Levels {
            values: vec![0; BUCKETS],
            filled: vec![false; BUCKETS],
        };
        assert!(bar_heights(&unknown, 10).iter().all(Option::is_none));
        // Half the RMS is 6 dB down: under a third of the 20 dB range lower.
        let mut values = vec![255; BUCKETS];
        values[BUCKETS / 2..].fill(180);
        let bars = bar_heights(&Levels::complete(values), 2);
        assert!((bars[1].unwrap() - 0.7).abs() < 0.02, "{bars:?}");
    }

    #[test]
    fn the_shared_picture_fills_while_on_and_forgets_when_off() {
        let wave = Arc::new(SeekWave::default());
        let uri = "spotify:track:a";
        wave.push(&tone(10, 1.0), 1.0);
        assert_eq!(
            wave.follow(uri, 1000, 0),
            0,
            "nothing is followed while off"
        );
        wave.set_enabled(true);
        let generation = wave.follow(uri, 1000, 0);
        assert_eq!(wave.follow(uri, 1000, 0), generation);
        wave.anchor(uri, 500);
        wave.push(&tone(441, 1.0), 1.0);
        let mut view = View::default();
        wave.refresh(uri, &mut view);
        let levels = view.levels.clone().unwrap();
        assert!(levels.filled[500] && levels.filled[509] && !levels.filled[0]);
        assert!(wave.wants(generation));
        wave.install(generation, vec![9; BUCKETS], false);
        assert!(!wave.wants(generation));
        wave.refresh(uri, &mut view);
        assert!(view.levels.as_ref().unwrap().is_complete());
        // Work for an earlier song is dropped.
        let next = wave.follow("spotify:track:b", 1000, 0);
        wave.install(generation, vec![1; BUCKETS], false);
        assert!(wave.wants(next));
        wave.refresh(uri, &mut view);
        assert!(view.levels.is_none());
        wave.set_enabled(false);
        assert!(!wave.wants(next));
    }
}
