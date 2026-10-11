//! Spicetify-style mods, run in a webview child process (fork only).
//!
//! The app starts this binary with `--mods-child`, the same way it starts
//! MilkDrop: the browser engine lives in that process, never in the app's.
//! The app writes the playing item to the child's stdin as one JSON object
//! per line; the child writes commands from the page to its stdout the same
//! way. `shim.js` turns both into the `Spicetify` API that mods expect.
//!
//! Everything here is fork code. Upstream files only call [`sync`] and
//! [`child::Args::parse`], so merging upstream rarely touches this folder.

pub mod child;
pub mod proxy;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::Mutex;
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
    let state = app.now_playing().map(|now| State {
        saved: app.is_saved(&now.uri),
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
    let written = replies
        .iter()
        .try_for_each(|reply| host.write(&Message::Reply(reply)))
        .and_then(|()| host.send(state));
    if written.is_err() || host.process.try_wait().ok().flatten().is_some() {
        let Slot::Running(mut host) = std::mem::replace(&mut *slot, Slot::Off) else {
            unreachable!("the slot was just matched as running");
        };
        let _ = host.process.kill();
        let _ = host.process.wait();
    }
}

impl Host {
    fn start(dir: &Path, data: &Path, ctx: egui::Context) -> Option<Self> {
        let exe = std::env::current_exe()
            .inspect_err(|error| log::warn!("Mods: could not find the program to run: {error}"))
            .ok()?;
        let mut process = Process::new(exe)
            .arg("--mods-child")
            .arg("--mods-dir")
            .arg(dir)
            .arg("--mods-data")
            .arg(data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
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
