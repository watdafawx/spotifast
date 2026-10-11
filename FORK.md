# Spotifast + mods (fork)

This fork adds Spicetify-style mods to [crmne/spotifast](https://github.com/crmne/spotifast).
Upstream won't take it (its AGENTS.md rules out a browser engine), so it
lives here and follows upstream daily.

## How it works

Built with `--features mods`, the app starts itself again with
`--mods-child`. That process opens a window with a webview (WebView2 on
Windows) and loads `index.html` from the mods folder, or a built-in demo
page. The app writes the playing item to the child's stdin as JSON lines;
the page's commands come back on stdout and become ordinary `Action`s. The
browser engine never loads into the app's own process.

`src/mods/shim.js` gives the page a `Spicetify` object (`Player`,
`LocalStorage`) built on that bridge, so Spicetify mods run unchanged as far
as the shim reaches.

- Mods folder: `mods` in the config folder (Windows: `%APPDATA%\paolino\spotifast\config\mods`)
- Webview data: `mods-webview` in the cache folder
- Devtools: right-click → Inspect in the mods window

## Keeping up with upstream

Everything fork-specific lives in new files: `src/mods/`, this file and
`.github/workflows/sync-upstream.yml`. Upstream files carry only these
`cfg(feature = "mods")` hooks, 12 lines in all:

| File | Hook |
|---|---|
| `Cargo.toml` | `mods` feature, `wry` dependency |
| `src/lib.rs` | `pub mod mods;` |
| `src/entrypoint.rs` | `--mods-child` dispatch, next to MilkDrop's |
| `src/app.rs` | `crate::mods::sync(self, ctx)` at the top of `sync_media_controls` |

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
