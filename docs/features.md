# Swell: feature scope

Target: everything High Tide 1.6 offers, minus its bugs, plus what a mature
music client is expected to have. Items marked **(new)** are beyond High Tide.

## Navigation and pages
- Sidebar navigation: Home, Explore, Search, Collection (Tracks, Albums,
  Artists, Playlists, Mixes & Radios), plus the user's playlists listed
  directly in the sidebar **(new)**.
- Home: personalised v2 feed (`v2/home/feed/static`): shortcuts, horizontal
  carousels (mixes, albums, artists, playlists), track lists, "View all"
  pages (`v2/home/pages/{id}/view-all`), more modules by cursor.
- Explore: `v1/pages/explore` with page links, genre/mood sub-pages.
- Search: live as you type (debounced) **(new)**, filters All / Tracks /
  Albums / Artists / Playlists **(new)**, top hit, tracks as a real track list,
  paging per type **(new)**.
- Album: header (cover, title, artists, year, track count, duration, quality
  badges), play / shuffle / favourite / share / more, tracks grouped by disc,
  review and credits.
- Artist: header (picture, name), play / shuffle (all top tracks, not just 5),
  follow, radio, share; top tracks, albums, EPs & singles, appears on, similar
  artists, artist mixes, bio.
- Playlist: header, tracks; own playlists can be renamed, re-described, made
  public/private, deleted, tracks removed and reordered **(new: reorder)**.
- Mix and track/artist radio pages.
- Recently played / activity page.
- "More" pages that page with limit/offset as you scroll.
- `tidal://` and `https://tidal.com/...` links open the right page **(new: web links)**.

## Collection
- Favourite tracks (virtualised, cached in SQLite, validated with one request)
  with filter-as-you-type and sort (date added, title, artist, album,
  duration) **(new)**.
- Favourite albums, artists, playlists (own + favourited), mixes.
- Favourite state everywhere from one `favorites/ids` request **(new)**;
  heart buttons that work for tracks, albums, artists, playlists and mixes.

## Playback
- Play from any list (album, playlist, mix, radio, top tracks, search
  results, favourites); the rest of a long list loads in the background.
- Queue: play next, add to queue (single tracks and whole albums/playlists
  **(new)**), view, jump to a row, remove, reorder, clear **(new)**.
- Shuffle, repeat off / all / one, previous restarts after 3 s.
- Gapless playback with the next stream prepared ahead.
- ReplayGain normalisation (track/album mode) as an option; off by default
  to keep the output bit-perfect.
- Volume, seek, keyboard control.
- Robustness: expired stream URLs reloaded at the same position, network
  errors retried, unavailable tracks skipped.
- Inhibit suspend while playing.
- Session restore: queue, position, shuffle and repeat **(new: position, queue)**.
- Sleep timer **(new)**.

## Now playing
- Full view with large cover, title, artist/album links, quality label
  (codec, bit depth / sample rate as actually delivered), heart, radio, menu.
- Synced lyrics: current line highlighted, auto-scroll, click to seek; plain
  lyrics when not synced.
- Queue view.

## Integrations
- MPRIS (all methods including Raise/Quit, metadata with art), media keys.
- Desktop notification on track change (optional) **(new)**.
- Run in background (hide on close) with the background portal.
- Copy share link.

## Preferences
- Audio quality (up to what the login allows), normalisation, gapless,
  output device (automatic / specific PipeWire/ALSA sink).
- Run in background, notifications.
- Cache: size used, clear covers / library cache **(new)**.
- Blocked artists/tracks management.
- Keyboard shortcuts dialog listing every shortcut.

## Keyboard
Space play/pause, Ctrl+→/← next/previous, Shift+→/← seek ±10 s,
Ctrl+↑/↓ volume, Ctrl+F search, Ctrl+L queue, Ctrl+, preferences,
Ctrl+? shortcuts, Alt+← back, Ctrl+Q quit, Ctrl+W close window.

## Known High Tide weaknesses to avoid
- No click-to-jump/remove/reorder in the queue; search only on Enter; artist
  play uses 5 tracks; mix favourites do nothing; context menus only on tracks;
  missing accessible names; volume in 10 % steps; MPRIS Raise/Quit broken.
