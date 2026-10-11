# Spotifast + mods (fork)

This fork adds Spicetify-style mods to [crmne/spotifast](https://github.com/crmne/spotifast).
Upstream won't take it (its AGENTS.md rules out a browser engine), so it
lives here and follows upstream daily.

## How it works

Built with `--features mods`, the app starts itself again with
`--mods-child`. That process opens a webview (WebView2 on Windows) and
loads `index.html` from the mods folder, or a built-in loader. On Windows
its window is a child of the app's window and takes over the lyrics view:
the lyrics side panel and full-screen lyrics show the mod instead of
Spotifast's own lyrics. The app tells the child where that area is each
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
| `Platform.LibraryAPI.contains` | the playing song's Liked state, sent with the player state |
| `Platform.LibraryAPI.add` / `remove` | the app's own `ToggleSaved` action |
| `GraphQL.Request` of `fetchExtractedColorForTrackEntity` / `getDynamicColorsByUris` for the playing song | the cover colour Spotifast tints with (`App::tint_for`), sent with the player state |

Anything else (other `GraphQL` queries, `Platform.RequestBuilder`, other
Cosmos URLs) rejects, so mods take their fallbacks.

With no `index.html` in the mods folder, the built-in loader
(`src/mods/loader.html`) runs the first Spicetify custom app it finds there
(or `?app=<name>`) the way Spicetify does: `subfiles_extension` as global
scripts, then `index.js` and `subfiles` in one function whose `render` is
the app's component. Buttons a mod adds to Spotify's menus or playbar show
in a toolbar at the top right. React 18.3.1 and Mousetrap come from
jsDelivr.

### ivLyrics

Copy the ivLyrics folder (the one with `manifest.json`) into the mods
folder as `mods/ivLyrics`. Lyrics, translation addons and its settings
(toolbar → "ivLyrics config") work, and so do its Spotify lyrics provider
(with local playback signed in), the Spotify profile, the like button and
album colours. Demo mode (`--demo`) shows no album colours: the app drops
backend events there, cover colours included.

- Mods folder: `mods` in the config folder (Windows: `%APPDATA%\paolino\spotifast\config\mods`)
- Webview data: `mods-webview` in the cache folder
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
| `src/ui/lyrics.rs` | `mods::place(ui)` at the top of the side panel's contents, and in full screen below the header |
| `src/app.rs` | `crate::mods::sync(self, ctx)` at the top of `sync_media_controls` |
| `src/backend.rs` | `Command::Mods` and its arm, which runs `mods::proxy::serve` with the engine and API gateway |

Keep it that way: anything new goes in `src/mods/`, reading `App`'s public
fields (`now_playing()`, `actions`, `dirs`) rather than adding to `App`.

The `Sync upstream` workflow merges `upstream/main` daily, builds, runs the
mods tests, and pushes. A `Cargo.lock` clash is resolved by taking
upstream's lockfile and letting cargo add the fork's crates back; any other
conflict opens an issue. By hand:

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
