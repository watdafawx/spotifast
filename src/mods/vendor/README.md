# Vendored for the mods loader (fork only)

Served from the binary at `/vendor/…` by `src/mods/child.rs`, so mods load
without a CDN. Fetched with `npm pack`, which checks each tarball against
the registry's integrity hash.

| File | Package | Licence |
|---|---|---|
| `react.production.min.js` | react 18.3.1 (`umd/`) | MIT, `react.LICENSE` |
| `react-dom.production.min.js` | react-dom 18.3.1 (`umd/`) | MIT, `react.LICENSE` (same text) |
| `mousetrap.min.js` | mousetrap 1.6.5 | Apache-2.0, `mousetrap.LICENSE` |

React 18.3.1 is the last release with UMD builds.
