//! Spicetify-style mods, run in a webview child process (fork only).
//!
//! The app starts this binary with `--mods-child`, the same way it starts
//! MilkDrop: the browser engine lives in that process, never in the app's.
//! The app writes the playing item to the child's stdin as one JSON object
//! per line; the child writes commands from the page to its stdout the same
//! way. `shim.js` turns both into the `Spicetify` API that mods expect.
//!
//! On Windows the mods window is a child of the app's window, shown where
//! the lyrics view would be: the lyrics view calls [`place`] to reserve its
//! area, and [`sync`] tells the child where that is, or to hide.
//!
//! Everything here is fork code. Upstream files only call [`sync`],
//! [`place`], [`attach_window`] and [`child::Args::parse`], so merging
//! upstream rarely touches this folder.

pub mod child;
pub mod proxy;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::backend::Command as BackendCommand;
use crate::model::Action;
use crate::player::RepeatMode;

/// Even while nothing changes, the page hears from the app this often, so
/// its clock never drifts far from the player's.
const RESEND: Duration = Duration::from_secs(5);
/// A position this far from where the page would have it is a seek.
const SEEK_SLACK_MS: i64 = 1_000;

/// The playing item as mods see it.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct State {
    uri: String,
    title: String,
    artists: Vec<Artist>,
    album: String,
    album_uri: Option<String>,
    art_url: Option<String>,
    duration_ms: u32,
    position_ms: u32,
    playing: bool,
    shuffle: bool,
    repeat: &'static str,
    volume_percent: u8,
    /// In the account's Liked Songs; `None` while unknown.
    saved: Option<bool>,
    /// The cover's colour as `#rrggbb`, the one Spotifast tints with;
    /// `None` until it has been worked out.
    color: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct Artist {
    name: String,
    uri: Option<String>,
}

/// A line to the page: `{"state": …}` or `{"reply": …}`. The child keeps
/// the latest state line for a page that loads later.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Message<'a> {
    State(Option<&'a State>),
    Reply(&'a proxy::Reply),
    /// Where the child's window goes in the app's window, or `None` to hide
    /// it. Handled by the child, not the page.
    Place(Option<Placement>),
}

/// A rectangle in the app window's client area, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// What a page may ask of the app.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Command {
    TogglePlay,
    Next,
    Previous,
    Seek {
        ms: u32,
    },
    Volume {
        percent: u8,
    },
    Shuffle {
        on: bool,
    },
    /// Like or unlike a song.
    SetSaved {
        uri: String,
        saved: bool,
    },
    /// A Spotify request, answered with a reply carrying the same id.
    Request {
        id: u64,
        call: proxy::Call,
    },
    /// An error on the page, for the app's log.
    Log {
        level: String,
        message: String,
    },
}

impl Command {
    fn action(self) -> Option<Action> {
        Some(match self {
            Command::TogglePlay => Action::TogglePlay,
            Command::Next => Action::Next,
            Command::Previous => Action::Previous,
            Command::Seek { ms } => Action::Seek(ms),
            Command::Volume { percent } => Action::SetVolume(percent.min(100)),
            Command::Shuffle { on } => Action::SetShuffle(on),
            Command::Log { level, message } => {
                log::warn!("Mods page {level}: {message}");
                return None;
            }
            // Handled in `sync`, which can see the app and the backend.
            Command::SetSaved { .. } | Command::Request { .. } => return None,
        })
    }
}

struct Host {
    process: Child,
    stdin: ChildStdin,
    commands: Receiver<Command>,
    reply_to: Sender<proxy::Reply>,
    replies: Receiver<proxy::Reply>,
    /// The mods folder holds a page to show, so the lyrics view hands over.
    has_page: bool,
    /// Where the window was last placed.
    placed: Option<Placement>,
    /// The app's place in its page history, to notice navigation; `None`
    /// before the first frame.
    page: Option<(usize, usize)>,
    /// The last state written and when; `None` before the first.
    sent: Option<(Option<State>, Instant)>,
}

enum Slot {
    NotStarted,
    Running(Box<Host>),
    /// The window was closed or never opened; stay off until the next launch.
    Off,
}

static HOST: Mutex<Slot> = Mutex::new(Slot::NotStarted);
/// The app's window, for the child to live in; 0 when there is none to use.
static PARENT: AtomicIsize = AtomicIsize::new(0);
/// The area the lyrics view last reserved, and in which egui pass.
static RESERVED: Mutex<Option<(Placement, u64)>> = Mutex::new(None);
/// The egui pass `sync` last ran in.
static SYNCED_PASS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Remembers the app's window so the mods window can be put inside it.
/// Windows only; elsewhere the mods window stays a window of its own.
pub fn attach_window(cc: &eframe::CreationContext<'_>) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(window) = handle.as_raw()
        {
            PARENT.store(window.hwnd.get(), Ordering::Relaxed);
        }
    }
    #[cfg(not(windows))]
    let _ = cc;
}

/// Whether a mod is running inside the app's window with a page to show,
/// so it takes over the lyrics view.
pub fn ready() -> bool {
    PARENT.load(Ordering::Relaxed) != 0
        && matches!(
            &*HOST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
            Slot::Running(host) if host.has_page
        )
}

/// Called where lyrics are shown (the page, full screen): when a mod is
/// [`ready`], reserves the rest of `ui` for it and returns true, and the
/// caller draws nothing there itself.
pub fn place(ui: &mut egui::Ui) -> bool {
    if !ready() {
        return false;
    }
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, egui::Sense::hover());
    let scale = ui.ctx().pixels_per_point();
    let min = (rect.min.to_vec2() * scale).round();
    let size = (rect.size() * scale).round();
    let placement = Placement {
        x: min.x as i32,
        y: min.y as i32,
        width: size.x.max(1.0) as u32,
        height: size.y.max(1.0) as u32,
    };
    *RESERVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some((placement, ui.ctx().cumulative_pass_nr()));
    true
}
/// Once per frame: start the mods window, send it the playing item, and turn
/// its commands into actions.
pub fn sync(app: &mut App, ctx: &egui::Context) {
    let mut slot = HOST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if matches!(*slot, Slot::NotStarted) {
        *slot = match Host::start(
            &app.dirs.config.join("mods"),
            &app.dirs.cache.join("mods-webview"),
            ctx.clone(),
        ) {
            Some(host) => Slot::Running(Box::new(host)),
            None => Slot::Off,
        };
    }
    let Slot::Running(host) = &mut *slot else {
        return;
    };
    for command in host.commands.try_iter() {
        match command {
            Command::Request { id, call } => {
                app.backend
                    .send(BackendCommand::Mods(Box::new(proxy::Request {
                        id,
                        call,
                        reply: host.reply_to.clone(),
                        wake: ctx.clone(),
                    })))
            }
            Command::SetSaved { uri, saved } => {
                if app.is_saved(&uri).unwrap_or(false) != saved {
                    app.actions.push(Action::ToggleSaved(uri));
                }
            }
            command => app.actions.extend(command.action()),
        }
    }
    let replies: Vec<_> = host.replies.try_iter().collect();
    // Lyrics shown as the page give way when the app goes to another page,
    // the way any Spotify page does.
    let page = (app.history_index, app.history.len());
    let navigated = host.page.replace(page).is_some_and(|before| before != page);
    if navigated && app.show_lyrics_panel && host.has_page && PARENT.load(Ordering::Relaxed) != 0 {
        app.actions.push(Action::ToggleLyricsPanel);
    }
    let state = app.now_playing().map(|now| State {
        saved: app.is_saved(&now.uri),
        // The same cover and colour as the player bar's tint, so it is
        // usually worked out already.
        color: app
            .tint_for(now.art_small.as_deref().or(now.art_url.as_deref()))
            .map(|color| format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())),
        uri: now.uri,
        title: now.title,
        artists: now
            .artists
            .into_iter()
            .map(|artist| Artist {
                uri: artist.id.map(|id| format!("spotify:artist:{id}")),
                name: artist.name,
            })
            .collect(),
        album: now.album_name,
        album_uri: now.album_id.map(|id| format!("spotify:album:{id}")),
        art_url: now.art_url,
        duration_ms: now.duration_ms,
        position_ms: now.position_ms,
        playing: now.playing,
        shuffle: now.shuffle,
        repeat: match now.repeat {
            RepeatMode::Off => "off",
            RepeatMode::Track => "track",
            RepeatMode::Context => "context",
        },
        volume_percent: now.volume_percent,
    });
    // eframe syncs twice a pass: first in `logic`, before the interface
    // draws, then after it. Before, the last pass's reservation stands;
    // after, only this pass's does, so a closed lyrics view hides the window
    // at once. Hiding and showing every pass would flicker, and WebView2
    // stops and restarts drawing each time.
    let pass = ctx.cumulative_pass_nr();
    let after_interface = SYNCED_PASS.swap(pass, Ordering::Relaxed) == pass;
    let reserved = match *RESERVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
    {
        Some((placement, at)) if at == pass || (at + 1 == pass && !after_interface) => {
            Some(placement)
        }
        _ => None,
    };
    let written = replies
        .iter()
        .try_for_each(|reply| host.write(&Message::Reply(reply)))
        .and_then(|()| {
            if reserved == host.placed {
                return Ok(());
            }
            host.placed = reserved;
            host.write(&Message::Place(reserved))
        })
        .and_then(|()| host.send(state));
    if written.is_err() || host.process.try_wait().ok().flatten().is_some() {
        let Slot::Running(mut host) = std::mem::replace(&mut *slot, Slot::Off) else {
            unreachable!("the slot was just matched as running");
        };
        let _ = host.process.kill();
        let _ = host.process.wait();
    }
}

/// An `index.html`, or a folder with a Spicetify `manifest.json`.
fn has_page(dir: &Path) -> bool {
    dir.join("index.html").is_file()
        || std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.path().join("manifest.json").is_file())
}

impl Host {
    fn start(dir: &Path, data: &Path, ctx: egui::Context) -> Option<Self> {
        let exe = std::env::current_exe()
            .inspect_err(|error| log::warn!("Mods: could not find the program to run: {error}"))
            .ok()?;
        let mut process = Process::new(exe);
        process
            .arg("--mods-child")
            .arg("--mods-dir")
            .arg(dir)
            .arg("--mods-data")
            .arg(data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let parent = PARENT.load(Ordering::Relaxed);
        if parent != 0 {
            process.arg("--mods-parent").arg(parent.to_string());
        }
        let mut process = process
            .spawn()
            .inspect_err(|error| log::warn!("Mods: could not start the window: {error}"))
            .ok()?;
        let stdin = process.stdin.take()?;
        let stdout = process.stdout.take()?;
        let (sender, commands) = channel();
        let (reply_to, replies) = channel();
        std::thread::Builder::new()
            .name("mods-commands".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match serde_json::from_str(&line) {
                        Ok(command) => {
                            if sender.send(command).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        Err(error) => log::debug!("Mods: ignored a command: {error}"),
                    }
                }
            })
            .ok()?;
        Some(Self {
            process,
            stdin,
            commands,
            reply_to,
            replies,
            has_page: has_page(dir),
            placed: None,
            page: None,
            sent: None,
        })
    }

    /// Writes the state when it changed in more than the clock's ordinary
    /// ticking, so the page is not woken every frame.
    fn send(&mut self, state: Option<State>) -> std::io::Result<()> {
        let now = Instant::now();
        let due = match (&self.sent, &state) {
            (None, _) => true,
            (Some((None, _)), None) => false,
            (Some((Some(sent), at)), Some(state)) => {
                let elapsed = if sent.playing {
                    now.duration_since(*at).as_millis() as i64
                } else {
                    0
                };
                let expected = i64::from(sent.position_ms) + elapsed;
                let moved = (i64::from(state.position_ms) - expected).abs() > SEEK_SLACK_MS;
                let rest_changed = State {
                    position_ms: 0,
                    ..sent.clone()
                } != State {
                    position_ms: 0,
                    ..state.clone()
                };
                moved || rest_changed || now.duration_since(*at) >= RESEND
            }
            (Some(_), _) => true,
        };
        if !due {
            return Ok(());
        }
        self.write(&Message::State(state.as_ref()))?;
        self.sent = Some((state, now));
        Ok(())
    }

    fn write(&mut self, message: &Message) -> std::io::Result<()> {
        let line = serde_json::to_string(message)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines the shim writes, as the bridge test page produced them.
    #[test]
    fn page_commands_become_actions() {
        let action = |line: &str| {
            serde_json::from_str::<Command>(line)
                .ok()
                .and_then(Command::action)
                .map(|action| format!("{action:?}"))
        };
        assert_eq!(
            action(r#"{"cmd":"toggle_play"}"#).as_deref(),
            Some("TogglePlay")
        );
        assert_eq!(
            action(r#"{"cmd":"seek","ms":100000}"#).as_deref(),
            Some("Seek(100000)")
        );
        assert_eq!(
            action(r#"{"cmd":"volume","percent":250}"#).as_deref(),
            Some("SetVolume(100)")
        );
        assert_eq!(action(r#"{"cmd":"volume","percent":300}"#), None);
        assert_eq!(
            action(r#"{"cmd":"volume","percent":42}"#).as_deref(),
            Some("SetVolume(42)")
        );
        assert_eq!(action(r#"{"cmd":"echo","title":"x"}"#), None);
        assert_eq!(
            action(r#"{"cmd":"log","level":"error","message":"x"}"#),
            None
        );
        let request = serde_json::from_str::<Command>(
            r#"{"cmd":"request","id":7,"call":{"kind":"lyrics","uri":"spotify:track:x"}}"#,
        );
        assert!(matches!(
            request,
            Ok(Command::Request {
                id: 7,
                call: proxy::Call::Lyrics { .. }
            })
        ));
        // Only listed calls exist; a page cannot ask for anything else.
        assert!(
            serde_json::from_str::<Command>(r#"{"cmd":"request","id":7,"call":{"kind":"token"}}"#)
                .is_err()
        );
    }
}
