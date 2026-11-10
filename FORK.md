# Spotifast + mods (fork)

This fork adds Spicetify-style mods to [crmne/spotifast](https://github.com/crmne/spotifast).
Upstream won't take it (its AGENTS.md rules out a browser engine), so it
lives here as a stack of patches on upstream, put back on top of it daily
(see "Keeping up with upstream").

## How it works

Built with `--features mods`, the app starts itself again with
`--mods-child`. That process opens a webview (WebView2 on Windows) and
loads `index.html` from the mods folder, or a built-in loader. On Windows
its window is a child of the app's window, shown as a page of its own: a
button beside Lyrics in the player bar (sparkles, named after the mod)
opens it below the top bar, as ivLyrics looks in Spotify. Spotifast's own
lyrics panel stays on its Lyrics button. With the mod's page open,
full-screen lyrics show the mod below the header. Back, or going to
another page, closes it. The mods process starts the first time the page is shown
and stops 30 seconds after nothing shows it, freeing its webview (about a
gigabyte with ivLyrics); showing it again starts it afresh. The app tells the child where that area is each
time it changes and hides it when no lyrics view is open. If the child
fails, Spotifast's own lyrics come back. Elsewhere it is a window of its
own. The app writes the playing item to the child's stdin as JSON lines;
the page's commands come back on stdout and become ordinary `Action`s. The
browser engine never loads into the app's own process.

`src/fork/mods/shim.js` gives the page a `Spicetify` object built on that
bridge, so Spicetify mods run unchanged as far as the shim reaches.

Spotify requests a mod would make with Spotify's own signed-in client go
through the app instead (`src/fork/mods/proxy.rs`): the page names a call, the
backend makes it with the app's grants, and only the answer comes back, so
no token reaches a page. The calls are a fixed list:

| Page asks for | Answered by |
|---|---|
| `CosmosAsync.get` of `spclient…/color-lyrics/v2/track/<id>` | `Engine::lyrics_json` (local playback session) |
| `CosmosAsync.get` of `api.spotify.com/v1/me` | `ApiClient::me` |
| `Platform.RequestBuilder` of `spclient…/metadata/4/track/<gid>` | the track's ISRC from librespot's metadata, as `{isrc}` (ivLyrics' video backgrounds) |
| `Platform.RequestBuilder` of the lyrics or `/v1/me` URLs above | the same as through `CosmosAsync.get` |
| `Platform.LibraryAPI.contains` | the playing song's Liked state, sent with the player state |
| `Platform.LibraryAPI.add` / `remove` | the app's own `ToggleSaved` action |
| `GraphQL.Request` of `fetchExtractedColorForTrackEntity` / `getDynamicColorsByUris` for the playing song | the cover colour Spotifast tints with (`App::tint_for`), sent with the player state |

Anything else (other `GraphQL` queries, other Spotify URLs) rejects, or
answers 501 through `RequestBuilder`, so mods take their fallbacks.

With no `index.html` in the mods folder, the built-in loader
(`src/fork/mods/loader.html`) runs the first Spicetify custom app it finds there
(or `?app=<name>`) the way Spicetify does: `subfiles_extension` as global
scripts, then `index.js` and `subfiles` in one function whose `render` is
the app's component. Buttons a mod adds to Spotify's menus or playbar show
in a toolbar at the top right. React 18.3.1 and Mousetrap 1.6.5 are
served from the binary (`src/fork/mods/vendor/`), so the loader needs no CDN.

### ivLyrics

Off by default since the native lyrics page replaced it: turn it on in
Settings → Lyrics extras → ivLyrics (web mod). While off, its player-bar
button is hidden and the webview never starts.

Copy the ivLyrics folder (the one with `manifest.json`) into the mods
folder as `mods/ivLyrics`. Lyrics, translation addons and its settings
(toolbar → "ivLyrics config") work, and so do its Spotify lyrics provider
(with local playback signed in), the Spotify profile, the like button and
album colours. Demo mode (`--demo`) shows no album colours: the app drops
backend events there, cover colours included.

Links the page opens in a new window (`target="_blank"`, `window.open`)
go to the system browser; other schemes are dropped. ivLyrics' Update
button opens its update page there, and the app then closes the mod's
page, goes Home and stops the mods process, so the updater can rewrite
the mod's files.

- Mods folder: `mods` in the config folder (Windows: `%APPDATA%\paolino\spotifast\config\mods`)
- Webview data, including the mods' settings: `mods-webview` in the data
  folder (Windows: `%LOCALAPPDATA%\paolino\spotifast\data\mods-webview`);
  one in the cache folder from an older build moves there once
- Carrying settings over: put `localstorage-import.json` (`{"key": "value"}`)
  in the mods folder; the loader writes it into the page's storage before
  the app starts, then deletes it. For ivLyrics this brings what its own
  Export leaves out, such as AI and lyrics providers.
- Devtools: right-click → Inspect in the mods window

## Native replacements for Spicetify extensions

Spicetify extensions edit Spotify's own interface, so they cannot run in
the mods webview. The fork rebuilds the ones worth keeping natively:

- **Genres in the player bar.** A dim line under the playing song's artist
  names shows up to three of its artists' genres, the first artist's first
  (`App::playing_genres`). Spotify no longer gives genres to apps outside
  its own clients: the Web API answers `"genres": []` for every artist and
  librespot's artist metadata dropped the field. So the backend
  (`src/fork/genres.rs`, `Command::ArtistGenres`) finds the artist on
  [MusicBrainz](https://musicbrainz.org) by its Spotify URL and takes its
  most voted genres, one request a second as MusicBrainz asks. When
  MusicBrainz has none, [Wikidata](https://www.wikidata.org) is asked for
  the artist with the same Spotify ID (property P1902, genre P136), then
  MusicBrainz is searched by name, taking only a single exact-name match
  with a near-perfect score (its genres, or tags with two votes). When
  none has any and a Last.fm API key is saved, Last.fm's top tags for the
  artist's name fill in, with tags such as "seen live" left out.
  Answers are kept in `genres` in the cache folder for 30 days (misses for
  3, or until a Last.fm key is saved); a failed lookup is tried again after
  five minutes. When nobody knows the artist the line is hidden. Settings →
  Genres turns the line off (`player_bar_genres`, on by default) and takes
  the Last.fm key, which is kept in the credential store with the AI keys.
  Genres Spotify does list (artist pages of apps it still serves) are used
  as they are.

## Lyrics translation and pronunciation (native)

The first step in replacing the ivLyrics webview with native code, and
not behind the `mods` feature. Under each line of Spotifast's own lyrics
(panel and full screen) it shows a pronunciation line in the Latin
alphabet and a translation line, smaller and dimmer, following the
highlight and autoscroll. Settings, under **Lyrics translation**: both
switches, the target language (default: the interface language), the
provider (Gemini, or any OpenAI-compatible endpoint: OpenRouter, OpenAI,
Groq) with its model, and the API key. With a key saved the model is
picked from the provider's own list (Gemini `models`, OpenAI-compatible
`/models`), with a switch to show only free models (OpenRouter marks
them) that is kept in the settings. **Fall back to other models** (on by
default): when a request fails, the other free models of that list are
tried (up to four, with the free filter on), then the other provider with
its own model, if its key is saved. Lyrics go to each one tried.

- One request per song with every distinct line (a chorus is sent once),
  asking for strict JSON: `{"language", "lines": [{"i", "pronunciation",
  "translation"}]}` (`src/fork/lyrics_ai.rs`, parsed leniently).
- Pronunciation is skipped for lyrics already in the Latin alphabet, and
  translation for lyrics already in the target language (a script and
  common-word check before asking, then the language the model reports).
- Answers are cached in `lyrics/extras` in the cache folder, keyed by the
  track, target language and words, for 180 days. Replaying asks nothing.
- Keys live in the platform credential store (`Slot::AiKeys`), never in
  `settings.json` or the log. The request runs on the backend runtime; the
  lyrics show at once and the extra lines fill in when the answer comes.

Upstream files touched: see "Hooks in upstream files" below.

## Music video behind full-screen lyrics (native)

`src/fork/lyrics_video.rs`, Windows only, not behind the `mods` feature. With
full-screen lyrics open, the backend searches YouTube through yt-dlp
("artist title official music video", picking an official upload about
as long as the song, never a lyric/audio/live upload) and caches the
pick per track for 7 days (1 day when nothing fits). mpv (`libmpv-2.dll`,
loaded at run time beside the app or from PATH) streams it without sound
and draws it with its OpenGL render API inside egui's paint callback
(`src/fork/lyrics_video/mpv.rs`); the cover shows until the first frame. It
follows the song's position, lined up by matching the video's captions
(its own or YouTube's automatic ones) to the timed lyrics (seek on
drift over 0.5 s, pause with the
song) and stops 30 s after full screen closes. Settings: Lyrics extras →
Video background. ivLyrics' community video service is not used: it
answers other apps with a warning video.

Video needs `libmpv-2.dll` and `yt-dlp.exe`. Each is looked for beside
`spotifast.exe` first, then on PATH; the fork's Windows releases bundle
both (see Releases), so nothing is installed by hand. When either is
missing, Settings → Lyrics extras and the lyrics page's Video panel say
which, and the switch is not offered. A failed search (most often an
out-of-date yt-dlp after a YouTube change) or mpv failing to start shows a
quiet line in that panel; the cover stays. yt-dlp ages faster than
releases: a newer `yt-dlp.exe` dropped in beside Spotifast, or
`yt-dlp.exe -U` run there, is used after a restart, and updates keep it
(the installer keeps a newer file version; a portable update replaces
only `spotifast.exe`). There is no auto-updater for it.

## Lyrics page (native)

`src/ui/fork/lyrics_page.rs` and `src/fork/lyrics_page.rs`, not behind the `mods`
feature. A player-bar button (TV icon) opens a page that takes the main
area below the top bar, as ivLyrics' page does: the music video (or the
softened cover) behind, blurred by mpv on the GPU (a two-pass Gaussian
user shader written to the cache folder) and dimmed, the lyrics centred
with their readings and meanings, a rail of controls at the right
(translation, pronunciation, video, blur/dimming/text size, centring,
full screen) and the song's lyrics offset at the bottom left (-100 ms to
+100 ms steps, per track URI, applied in every lyrics view). Optional looks, all off by default (the
appearance panel): the sung line Classic, in the cover's colour (Accent)
or with lines fading by distance (Fade); words lighting up as they are
sung (timed by Unison, unison.boidu.dev, the open crowdsourced API behind
Better Lyrics, when it has the song word by word: asked by ISRC, matched
to the shown lines by their letters, cached 30 days in `lyrics/words`;
else estimated from the line's length); and only the sung line showing, sliding in and out. Animations and effects, all optional (`src/ui/fork/lyrics_fx.rs`): how lines
enter (fade, rise, typewriter, zoom) and leave (fade, drift up, zoom) in
the single-line view, the sung line floating or pulsing, a glow on the
sung line and an outline on all, dots through instrumental breaks (on by
default), and slow colour blobs from the cover's colour when there is no
video. The sung line sits in the middle of the page. Back or any
navigation closes it. Settings live under `lyrics_page` in
`settings.json`.

## Other fork features

- **Waveform seek bar** (`src/fork/seekwave.rs`, `src/player/fork.rs`,
  `src/ui/player_bar/fork.rs`). Settings → Appearance → Waveform seek bar
  (`seek_waveform`, off by default) draws the seek bar as the song's
  waveform. It is worked out on this computer from the sound after the
  equalizer and before the volume: filled in while a song plays here, and
  completed from librespot's audio cache once the whole file is there (one
  extra audio key request for a song played at least five seconds). Each
  finished song is kept in `waveforms` in the cache folder, about 1 KB a
  song. Demo surfaces: `seek-waveform` (a song heard before) and
  `seek-waveform-filling` (one whose first part has played).
- **Collapsed Library** (`src/ui/sidebar/fork.rs`, #298 upstream). The
  Library icon collapses the Library to a strip of covers, names on hover;
  dragging the edge switches between the two (`sidebar_collapsed`).
- **Glass themes on Windows** (`src/fork/glass.rs`, `src/ui/fork/mod.rs`).
  The main window is created see-through with Windows 11's acrylic blur
  behind it. Opaque themes paint every pixel and look as before; themes
  whose colours have alpha (`contrib/themes/*Glass*.json`, `Frost`,
  `Smoked Acrylic`) show the blurred desktop, and a tinted header stays
  see-through.
- **Cover eviction fix** (`src/images.rs`, inline). Covers still on screen
  are never evicted from the texture cache.

Privacy: besides upstream's list (`docs/_reference/privacy.md`), the genre
line sends the playing song's artists' Spotify IDs to MusicBrainz and
Wikidata, and with a saved Last.fm key the artist's name to Last.fm; each
artist about once a month. Turn the line off in Settings → Genres to send
nothing. Lyrics extras send the lyrics to the AI provider whose key you
saved; the lyrics video asks YouTube (through yt-dlp) for the song's video.

## Keeping up with upstream

The fork is a stack of commits on top of `upstream/main`, never a merge.
Upstream's files carry only one-line hooks into fork-owned files, so the
stack applies to each new upstream without a person or an LLM resolving
anything.

### Where fork code lives

- `src/fork/`: everything that needs no private upstream items (genres,
  lyrics AI, lyrics page settings, lyrics video, mods, seekwave, word
  timing, glass), plus the fork's `State`, `Settings`, `Action`, `Event`
  and `Command` types.
- `src/ui/fork/`: the fork's own views (genres and lyrics extras settings,
  the lyrics page and its effects) and the hooks `src/ui/mod.rs` calls.
- `<upstream file>/fork.rs`: a child module of an upstream file, for fork
  code that needs that file's private items. Each has a `mod fork;` line in
  its parent: `src/app/fork.rs`, `src/backend/fork.rs`, `src/player/fork.rs`,
  `src/demo/fork.rs`, `src/ui/lyrics/fork.rs`, `src/ui/sidebar/fork.rs`,
  `src/ui/player_bar/fork.rs`, `src/ui/settings/fork.rs`,
  `src/ui/widgets/fork.rs`; tests in `src/app/tests/fork.rs` and
  `src/demo/tests/fork.rs`.

New fork work goes in those files. In an upstream file add at most a
one-line call, a field, or an enum variant, never a changed upstream line.

### Hooks in upstream files

| File | Hooks |
|---|---|
| `Cargo.toml` | `wry`, `librespot-audio`, two `windows-sys` features, the `mods` feature |
| `src/lib.rs` | `pub mod fork;` |
| `src/entrypoint.rs` | `fork::run_child()`, `fork::attach_window`, `fork::window_viewport`, `fork::SEE_THROUGH` in `clear_color` |
| `src/model.rs` | `Action::Fork` |
| `src/settings.rs` | `fork: crate::fork::Settings`, flattened, so its keys sit beside upstream's in `settings.json` |
| `src/app.rs` | `fork` field; `fork::init_tap`, `fork_lyrics_loaded`, `fork_event`, `fork::update_checked`, `fork_track_changed`, `request_playing_genres`, `fork_sync`, `fork_artist`, `fork_before_action`, `fork_action`; `mod fork;` twice |
| `src/backend.rs` | `Command::Fork`, `Event::Fork`, the worker's `fork` field, its dispatch arm, the AI keys grant arm, `mod fork;` |
| `src/credentials.rs` | the `AiKeys` slot and grant (inline, with tests) |
| `src/player.rs` | the engine's `seek_wave` field, its session, the event feed, `mod fork;` |
| `src/vis.rs`, `src/sink.rs` | the tap's `seek_wave` and its push; `waiting_for_track` made `pub(crate)` |
| `src/updates.rs` | mods builds update from `watdafawx/spotifast`, signed with the fork's key |
| `src/theme.rs` | the `Palette` and `LocateFixed` icons |
| `src/images.rs` | the cover eviction fix (inline) |
| `src/demo.rs` | `fork::populate`, `fork::apply_flag`, `mod fork;` twice |
| `src/ui/mod.rs` | `mod fork;`, `fork::min_width`, `fork::central`, `fork::glass_wash`, `fork::glass_blend` |
| `src/ui/lyrics.rs` | `mod fork;`, `fork::mods_page`, and the first lines of `contents`, `background`, `fullscreen_contents` handing over to the fork's copies |
| `src/ui/sidebar.rs` | `mod fork;`, `show` handing over to the fork's copy, `fork::collapse_button` |
| `src/ui/player_bar.rs` | `mod fork;`, genre line (4 lines), `fork::seek_rect`, `fork::seek_slider`, `fork::extras_buttons` |
| `src/ui/settings.rs` | `mod fork;`, `fork::appearance`, `fork::sections`, `fork::updates_link` |
| `src/ui/widgets.rs` | `pub mod fork;` |

### Copied functions

Where a fork feature reshapes a whole upstream function, the fork keeps its
own copy and upstream's function hands over at its first line
(`if fork::REPLACES { return fork::...; }`). Upstream's body stays,
compiled but unused, so its changes never conflict:

- `src/ui/lyrics.rs`: `contents`, `fullscreen_contents`, `background`
  (copies in `src/ui/lyrics/fork.rs`).
- `src/ui/sidebar.rs`: `show`, and the shelf building in `contents`
  (copies in `src/ui/sidebar/fork.rs`).
- `src/ui/widgets.rs`: `thin_slider`'s behaviour (`painted_slider` in
  `src/ui/widgets/fork.rs`).

The cost: an upstream fix to one of these reaches the fork only when it is
ported to the copy. The sync opens an issue labelled `sync` listing the
upstream commits that changed one, so nothing is missed.

### The daily sync

`Sync upstream` runs daily (and by hand). It cherry-picks the fork's
commits onto the new `upstream/main` with
[Mergiraf](https://mergiraf.org), a merge driver that understands Rust,
TOML and YAML, so edits that are only near each other merge cleanly. The
fork's commits never touch `Cargo.lock`: the top commit, "Cargo.lock for
the fork's crates (fork)", is dropped and made again by cargo each time.
A Windows job then builds with mods and runs the tests. Only then does
`mods` move (force-pushed, with a lease); its previous state is kept on
`mods-previous`.

### When the sync fails

`mods` stays where it was, and an issue "Upstream sync failed" (label
`sync`) links the run; its summary names the commit and files that clash.
To fix it by hand:

```bash
git fetch upstream origin
git checkout -B mods origin/mods
git rebase upstream/main        # resolve, keeping each hook a single line
cargo metadata --format-version 1 > /dev/null   # lockfile, if needed
cargo test --no-default-features --features mods,demo --lib
git push --force-with-lease origin mods
```

Resolve by keeping upstream's code and re-adding the fork's hook where the
code it hooked into went. If a hook keeps clashing, move more of the fork
change into a fork file. A rebase that drops the Cargo.lock commit is fine:
the next sync makes it again.

Upstream's own workflows (issue triage, releases, project board) also run
in a fork. Turn off the ones that don't apply under the fork's Settings →
Actions instead of deleting the files, so they never conflict.

## Releases

Upstream's `Release` stays off here: it signs with upstream's key and
publishes upstream's packages. `Cargo.toml` keeps upstream's version; the
fork's version is made at release time. A fork release of upstream's
`MAJOR.MINOR.PATCH` is `MAJOR.MINOR.(PATCH*100+n)`: upstream 0.17.1 gives
0.17.100, 0.17.101, ...; upstream 0.18.0 gives 0.18.0, 0.18.1, ... Each
sorts above the fork releases before it, so the updater offers it.

`Release mods` runs on every push to `mods` and after each sync. When
upstream's version has no fork release yet, it publishes n = 0. Run it by
hand to publish fork-only changes as the next n (or with `force` to replace
the latest). It builds the Windows x64 app with mods, packages the zip and
setup program the updater asks for (adding the video components to
`spotifast.iss` in the workflow), writes `checksums.txt`, signs it
(`checksums.txt.sig`) and publishes the release, tagging the `mods` commit
it was built from. Notes come from `packaging/release-notes/<tag>.md` when
that file exists.

The Windows zip and setup program also carry the video components, pinned
in the workflow's "Video components" step and checked against SHA-256
digests before packaging, so `checksums.txt` and its signature cover them:

| File | Version | Source | Licence |
| --- | --- | --- | --- |
| `libmpv-2.dll` | shinchiro mpv-dev x86_64 20261101 (mpv `48fca90e8c`) | [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake), SourceForge mirror as fallback | GPL (mpv is GPL 2+ or LGPL 2.1+; this build includes GPL libraries such as x264 and x265) |
| `yt-dlp.exe` | 2026.10.26 | [yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp/releases) | Unlicense (the bundled Python runtime and libraries keep their own licences) |

`packaging/windows/third-party-video.txt` ships beside them with the
licences and source links. To move to newer builds, change the six values
in that step, this table and that file together. A portable copy that
updates itself replaces only `spotifast.exe`, so a portable folder from
before the video components were bundled gets them from a fresh zip.

Builds with mods only accept updates signed with the fork's Ed25519 key
(`assets/update-public-key-mods.hex`). The private key is the
`MODS_UPDATE_SIGNING_KEY` repository secret (PEM, from
`openssl genpkey -algorithm ed25519`). A build made before the fork had its
own key checks upstream's key and refuses fork releases: install one fork
release by hand once.

## Build and test

```bash
cargo build --no-default-features --features mods
cargo test --no-default-features --features mods,demo --lib
```

The sync runs exactly that test command on Windows.

`--no-default-features` leaves out MilkDrop, which needs cmake and libclang;
`--features milkdrop,mods` builds both. Windows only for now: Linux needs
wry's GTK path.
