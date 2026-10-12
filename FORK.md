# Spotifast + mods (fork)

This fork adds Spicetify-style mods to [crmne/spotifast](https://github.com/crmne/spotifast).
Upstream won't take it (its AGENTS.md rules out a browser engine), so it
lives here and follows upstream daily.

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

`src/mods/shim.js` gives the page a `Spicetify` object built on that
bridge, so Spicetify mods run unchanged as far as the shim reaches.

Spotify requests a mod would make with Spotify's own signed-in client go
through the app instead (`src/mods/proxy.rs`): the page names a call, the
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
(`src/mods/loader.html`) runs the first Spicetify custom app it finds there
(or `?app=<name>`) the way Spicetify does: `subfiles_extension` as global
scripts, then `index.js` and `subfiles` in one function whose `render` is
the app's component. Buttons a mod adds to Spotify's menus or playbar show
in a toolbar at the top right. React 18.3.1 and Mousetrap 1.6.5 are
served from the binary (`src/mods/vendor/`), so the loader needs no CDN.

### ivLyrics

Copy the ivLyrics folder (the one with `manifest.json`) into the mods
folder as `mods/ivLyrics`. Lyrics, translation addons and its settings
(toolbar → "ivLyrics config") work, and so do its Spotify lyrics provider
(with local playback signed in), the Spotify profile, the like button and
album colours. Demo mode (`--demo`) shows no album colours: the app drops
backend events there, cover colours included.

- Mods folder: `mods` in the config folder (Windows: `%APPDATA%\paolino\spotifast\config\mods`)
- Webview data, including the mods' settings: `mods-webview` in the data
  folder (Windows: `%LOCALAPPDATA%\paolino\spotifast\data\mods-webview`);
  one in the cache folder from an older build moves there once
- Carrying settings over: put `localstorage-import.json` (`{"key": "value"}`)
  in the mods folder; the loader writes it into the page's storage before
  the app starts, then deletes it. For ivLyrics this brings what its own
  Export leaves out, such as AI and lyrics providers.
- Devtools: right-click → Inspect in the mods window

## Keeping up with upstream

Everything fork-specific lives in new files: `src/mods/`, this file and
`.github/workflows/sync-upstream.yml`. Upstream files carry only these
`cfg(feature = "mods")` hooks:

| File | Hook |
|---|---|
| `Cargo.toml` | `mods` feature, `wry` dependency |
| `src/lib.rs` | `pub mod mods;` |
| `src/entrypoint.rs` | `--mods-child` dispatch, next to MilkDrop's; `mods::attach_window(cc)` after the app attaches |
| `src/ui/lyrics.rs` | `mods::place(ui)` in full screen below the header while the mod's page is open |
| `src/ui/mod.rs` | `mods::place(ui)` in `central` after the top bar while the mod's page is open |
| `src/ui/player_bar.rs` | `mods::page_button` beside the Lyrics button |
| `src/app.rs` | `crate::mods::sync(self, ctx)` at the top of `sync_media_controls`; `Action::Back` closes the mod's page |
| `src/updates.rs` | builds with mods update from `watdafawx/spotifast` releases |
| `src/backend.rs` | `Command::Mods` and its arm, which runs `mods::proxy::serve` with the engine and API gateway |

One fork change is not behind the feature: **glass themes on Windows**.
The main window is created see-through with Windows 11's acrylic blur
behind it (`window::frosted_backdrop`, called from `entrypoint.rs`;
`windows-sys` gains `Win32_Graphics_Dwm` and `Win32_UI_Controls`). Opaque
themes paint every pixel and look as before; themes whose colours have
alpha (`contrib/themes/*Glass*.json`, `Frost`, `Smoked Acrylic`) show the
blurred desktop. `ui/mod.rs` keeps a tinted header see-through for them
(`blend` keeps the base's alpha; the page header fades to transparent).
These touch upstream code directly, so expect them in merge conflicts.

Keep it that way: anything new goes in `src/mods/`, reading `App`'s public
fields (`now_playing()`, `actions`, `dirs`) rather than adding to `App`.

The `Sync upstream` workflow merges `upstream/main` daily, builds, runs the
mods tests, and pushes; with nothing new upstream it stops there. A
`Cargo.lock` clash is resolved by taking upstream's lockfile and letting
cargo add the fork's crates back; any other conflict fails the run with
the conflicting files in its summary, and GitHub emails the owner. By
hand:

```bash
git fetch upstream && git merge upstream/main
```

Upstream's own workflows (issue triage, releases, project board) also
run in a fork. Turn off the ones that don't apply under the fork's
Settings → Actions instead of deleting the files, so they never conflict.

## Build and test

```bash
cargo build --no-default-features --features mods
cargo test --no-default-features --features mods --lib mods::
```

`--no-default-features` leaves out MilkDrop, which needs cmake and libclang;
`--features milkdrop,mods` builds both. Windows only for now: Linux needs
wry's GTK path.
