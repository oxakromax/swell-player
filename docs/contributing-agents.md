# Working on Swell (brief for parallel contributors)

Swell is a native TIDAL client for Linux: Rust edition 2024, gtk4-rs 0.11
(GTK 4.20 API), libadwaita 0.9 (API 1.8), GStreamer 0.25, tokio, reqwest.
GPL-3.0-or-later. Read first: `docs/features.md` (scope), `docs/design.md`
(the UI spec: follow it), and the code you build on.

## Architecture (already in place)
- `src/tidal/`: our own TIDAL API client. `Client` methods in
  `endpoints.rs` (albums, artists, playlists, mixes, lyrics, search,
  favourites, playlist editing...), lenient models in `models.rs`, editorial
  pages / home feed parsed into `Page`/`Section`/`Item` in `pages.rs`.
- `src/api.rs`: `Api` (derefs to `Client`) + `load_cached(base, path,
  fresh_for, parse)` = stale-while-revalidate from SQLite (use it for pages),
  `invalidate(prefix)`, `sync_favorite_tracks`, `stream(track_id)`.
- `src/runtime.rs`: ALL network/disk I/O runs on the tokio runtime:
  `runtime::spawn(async move {...}).await` from a `glib::spawn_future_local`
  task. Never block the GTK main thread.
- `src/covers.rs`: `ctx.covers.load(&Image::Cover(id) | Artist | Playlist |
  Url, pixels (= size × scale factor), list position, |texture| ...)` returns
  a `Ticket` to keep while waiting (drop = cancel).
- `src/model.rs`: `TrackInfo` (what lists and the queue hold), `From<Track>`.
- `src/playback/`: `Playback` facade (queue, transport, options, events via
  `subscribe`). The UI never touches GStreamer.
- `src/library.rs`: favourites (`is_favorite`, `set_favorite` optimistic),
  user playlists, favourite mixes, change notifications.
- `src/settings.rs`: JSON settings `Store` (`get`, `update`, `connect_changed`).
- `src/ui/context.rs`: `Ctx` shared by every page/widget: api, covers,
  playback, library, settings, toasts, `navigate(Route)`, `toast`,
  `toast_with_undo`, `report(msg, &err)` (signs out on expired session),
  `copy`, now-playing panel (`show_panel`, `toggle_panel`,
  `connect_panel_changed`).
- `src/ui/nav.rs`: `Route` + `Navigator`. Pages: `src/ui/pages/*.rs`, each
  `pub fn page(ctx: &Rc<Ctx>, ...) -> adw::NavigationPage`.

## Rules
- Only edit the files your brief says you own. If you need something
  elsewhere, work around it and list it in your report.
- Lists that can be long: `gtk::ListView`/`GridView` directly in a
  `ScrolledWindow`, fixed row heights, recycled rows, covers via the loader.
- Hold `Rc<Ctx>` weakly from closures stored in widgets (avoid cycles);
  abort/cancel async work when the page is destroyed.
- Accessibility: every icon-only button has a tooltip/accessible label.
- Style: match the surrounding code (doc comments on modules and non-obvious
  items, no noise comments). `cargo clippy --release` must be clean of new
  warnings in your files; `cargo test --release` must pass.
- Crates: latest stable versions; prefer none.
- Do not copy code from High Tide or tidalapi (LGPL/GPL elsewhere); the
  protocol knowledge is fine.

## Verifying your work visually (headless)
The user's screen is off; render the app off-screen:

    kwin_wayland --virtual --socket wayland-swell --width 1400 --height 900 --no-lockscreen &   # once; may already run
    WAYLAND_DISPLAY=wayland-swell SWELL_ROUTE=album:529955456 \
      SWELL_SCREENSHOT=/path/shot.png SWELL_SCREENSHOT_DELAY_MS=5000 \
      ./target/release/swell-player

`SWELL_SCREENSHOT_SIZES=1400x900,1000x760,400x800` shoots several widths
(`shot-WxH.png`). `SWELL_PANEL=playing|lyrics|queue` opens the panel. See
`src/ui/debug.rs` for routes (album:, artist:, playlist:, mix:, search:,
tracks, albums, ...). Look at the PNGs (Read tool) and iterate until the
screen matches docs/design.md. A real TIDAL session is in the keyring, so
real data loads. Useful ids: album 529955456 (YOASOBI, Hi-Res), artist
17275 (Eminem), track 529955457.

Do not leave processes running (other than the shared kwin_wayland).
Do not play audio for longer than a few seconds when testing playback.

## Finishing
Commit your work on your branch (small logical commits are fine) with a
message ending in:

    Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

Report: what you built, files touched, anything left undone or needing an
API you did not own, and known issues.

## What already exists (phase A, merged)
- Shell (`ui/window.rs`, `ui/nav.rs`, `ui/sidebar.rs`): per-section
  navigation stacks; `ctx.navigate(route)`. Section root pages should put
  `sidebar::show_sidebar_button(&ctx.window)` at the start and
  `sidebar::narrow_main_menu_button(&ctx.window)` at the end of their header
  bar. Ctrl+F focuses the first `gtk::SearchEntry` of the Search page.
  `ctx.toast_with_undo` supports Ctrl+Z. Window gets class `narrow` at
  ≤ 720sp.
- Widget kit (`ui/widgets/`):
  - `TrackList` (`track_list`): `new(ctx, Variant::{Cover,Numbered,Queue})`
    (own ScrolledWindow) / `embedded(ctx, variant)`; `widget()`, `view()`,
    `scrolled()`, `downgrade()`; `set_header/set_footer(Option<&W>)` (header
    scrolls with the list), `set_tracks/replace/append/insert/remove/
    move_track`, `len`, `track(i)`, `tracks()`; `set_source(Source)`,
    `set_album_artists`, `set_group_discs`, `set_compact`,
    `follow_page_compact()`, `scroll_to_top()`; hooks
    `connect_activated(Fn(usize))` (replaces default play),
    `set_remove_handler(Fn(usize))` ("Remove from This Playlist"),
    `set_reorderable(Fn(from,to))` (row already moved; move back on
    failure), `connect_near_end(Fn())` (paging); `compact_bin(child,
    Fn(bool)) -> adw::BreakpointBin` (one per page).
  - Drag and drop: `track_row::TrackDrag { track_ids, origin }`.
  - Sections: `section::section_widget(ctx, &Section) -> gtk::Widget`,
    `activate_item(ctx, &Item)`, `activate_target`, `tracks_section`,
    `short_track_list`, `view_all_route`.
  - `MediaCard::new(ctx, &Item, Option<&Rc<ItemMenu>>).widget()`,
    `media_card::{cover, Cover, avatar, attach_menu}`,
    `CarouselSection::new(ctx, title, subtitle, &[Item])` +
    `connect_view_all`; `PageHeader::{album, playlist, artist, mix}` +
    `connect_play/connect_shuffle/set_playable/set_compact/add_detail`;
    `QualityBadge::new(BadgeKind)`; `HeartButton::{new, follow, unlistened}`;
    `skeleton::{row, card, header, page}`; `status::{PageStack,
    error_status, empty_status, describe_error}`.
  - Grids of cards: `gtk::GridView` with CSS class `media-grid`.
- Menus (`ui/menus.rs`): `Subject::{Track{track,index}, Album, Artist,
  Playlist, Mix}`, `Subject::from_item`, `ItemMenu::new(ctx, parent)` +
  `popup/popup_at/popup_at_point/set_remove_handler/set_move_handler`;
  `play(ctx, &Subject, PlayMode)`, `play_tracks(ctx, tracks, start, source,
  strict)`, `shuffle_tracks`, `enqueue`, `TrackQuery::fetch`,
  `toast_with_action`, `play_track_id`.
- Dialogs: `dialogs::playlist::{add_to_playlist, choose_playlist,
  new_playlist(ctx, ids), edit_playlist(ctx, &Playlist), set_public,
  delete_playlist(ctx, &Playlist, on_deleted)}`;
  `dialogs::credits::{show, credits_page, show_text}`.
- Playback: see `src/playback/mod.rs` (format_details, sleep timer,
  save/restore session, MPRIS). `SWELL_PLAY=<id>[@secs]` plays at startup.
- Album and Artist pages (`ui/pages/{album,artist}.rs`) are the reference
  for how a page loads (`api.load_cached`), shows skeletons/errors and
  uses the kit. Copy their structure.
- More debug variables: `SWELL_SECTION`, `SWELL_DIALOG`, `SWELL_ACTIONS`,
  `SWELL_LOGIN=1`, route `history` (see `ui/debug.rs`).
