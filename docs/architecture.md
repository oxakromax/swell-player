# Architecture

How Swell is put together, for people reading or changing the code. The
feature scope is in [`features.md`](features.md) and the interface
specification in [`design.md`](design.md).

Swell is a single Rust binary (edition 2024) built on gtk4-rs,
libadwaita, GStreamer, Tokio and reqwest. Icons and translations are
embedded, so it runs from anywhere without being installed.

## Principles

- **No network or disk on the main thread.** GTK owns it; HTTP, SQLite,
  keyring access and image decoding run on a small Tokio runtime
  (`src/runtime.rs`). UI code calls
  `runtime::spawn(async move { … }).await` from a
  `glib::spawn_future_local` task.
- **Stale-while-revalidate.** Pages and the library are drawn from SQLite
  (`~/.cache/swell-player/library.sqlite3`) at once and refreshed in the
  background. Favourites are validated with one request, and tracks liked
  elsewhere are prepended without downloading the list again. Old answers
  and least-used artwork are pruned automatically.
- **Virtualised lists and lazy artwork.** Long lists are `gtk::ListView` /
  `gtk::GridView` with fixed row heights and recycled rows. Covers load only
  near the visible area, at the size shown, at most four at a time, nearest
  the middle of the viewport first, within a 64 MB budget of decoded
  textures.
- **The UI never touches GStreamer.** Playback is a facade with a queue,
  transport, options and an event stream.

## Modules

| Path | What it does |
|---|---|
| `src/tidal/` | Swell's own TIDAL API client. `Client` methods in `endpoints.rs` (albums, artists, playlists, mixes, lyrics, search, favourites, playlist editing…), lenient models in `models.rs` (a malformed item drops only itself), editorial pages and the home feed parsed into `Page`/`Section`/`Item` in `pages.rs`, sign-in flows in `auth.rs`. |
| `src/api.rs` | `Api` (derefs to `Client`) plus `load_cached(base, path, fresh_for, parse)` (stale-while-revalidate from SQLite), `invalidate(prefix)`, `sync_favorite_tracks`, `stream(track_id)`. |
| `src/cache.rs` | The SQLite cache of API answers and the library. |
| `src/covers.rs` | Artwork loader: `ctx.covers.load(&Image, pixels, position, callback)` returns a `Ticket` to keep while waiting (dropping it cancels). |
| `src/runtime.rs` | The Tokio runtime for all I/O. |
| `src/model.rs` | `TrackInfo`, what lists and the queue hold. |
| `src/playback/` | `Playback` facade over GStreamer `playbin3`: queue, transport, gapless handover, ReplayGain, output device, sleep timer, session save/restore, MPRIS, format details. |
| `src/library.rs` | Favourites (`is_favorite`, optimistic `set_favorite`), user playlists, favourite mixes, change notifications. |
| `src/settings.rs` | JSON settings store (`get`, `update`, `connect_changed`) in `~/.config/swell-player/settings.json`. |
| `src/i18n.rs` | Translations: embedded `.po` catalogues, `tr`/`trn`, TIDAL locale. See [`translating.md`](translating.md). |
| `src/ui/context.rs` | `Ctx`, shared by every page and widget: api, covers, playback, library, settings, toasts, `navigate(Route)`, `report(msg, &err)`, the now-playing panel. |
| `src/ui/nav.rs` | `Route` and `Navigator` (one navigation stack per section). |
| `src/ui/pages/` | One module per page; each exposes `page(ctx, …) -> adw::NavigationPage`. Album and Artist are the reference for loading, skeletons and error states. |
| `src/ui/widgets/` | The widget kit: `TrackList`, media cards, carousels, page headers, quality badges, heart buttons, skeletons, status pages. |
| `src/ui/menus.rs` | Context menus for every kind of object, and the play/enqueue helpers. |
| `src/ui/dialogs/` | Preferences, playlists, credits, About. |
| `src/ui/debug.rs` | Developer aids driven by environment variables (see [CONTRIBUTING](../CONTRIBUTING.md#debugging-aids)). |

## TIDAL client

- v1 and v2 endpoints over one HTTP/2 connection per host.
- Token refresh is shared by concurrent requests; only a refresh token that
  TIDAL rejects (`invalid_grant`) signs the user out, never network trouble.
- Retries only for idempotent requests.
- Playlist edits carry the ETag of the version they were made on, so an
  edit never silently overwrites changes made elsewhere.
- **DNS without stalls.** Names are resolved with
  [hickory](https://crates.io/crates/hickory-resolver): on some networks
  glibc's parallel A/AAAA lookups stall for 5 s.

## Playback

- GStreamer `playbin3`. Lossless streams are plain FLAC; Hi-Res streams are
  MPEG-DASH manifests, written to a private per-process directory under
  `$XDG_RUNTIME_DIR` and streamed by `dashdemux2`.
- The next track's stream is prepared ahead and handed over on
  `about-to-finish` for gapless playback.
- Expired stream URLs are reloaded at the same position; network drops are
  waited out and playback resumes where it stopped.
- With normalisation off and volume at 100 %, no element touches the
  samples: the stream's format reaches PipeWire unchanged.

## Data on disk

| Location | Contents |
|---|---|
| `~/.config/swell-player/settings.json` | Preferences, window size, volume. |
| `~/.cache/swell-player/` | `library.sqlite3` (cached API answers and the library) and `covers/` (artwork). Safe to delete. |
| `~/.local/state/swell-player/` | The saved queue and position, recent searches. |
| `$XDG_RUNTIME_DIR` | Hi-Res DASH manifests while playing (private, per process). |
| Keyring (Secret Service) | The TIDAL tokens. Never written to disk in plain text. |

Signing out deletes the tokens, the cache, the saved queue and the search
history.
