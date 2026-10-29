# Changelog

All notable changes to Swell are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.3.3] - 2026-10-29

### Fixed

- **Windows updates that stopped with the installer frozen:** an update
  waits for every program of the install to exit before it replaces
  anything, and gave up, keeping the old version, when one still ran
  30 s after Swell quit. After 10 s it now ends what outlived Swell: the
  session bus Swell started (`gdbus.exe`), unless another program uses
  it, GLib's and GStreamer's helpers, and a Swell that hangs on its way
  out. The fix is in the installer, so it already applies to the update
  to 1.3.3. The installer's log
  (`%LOCALAPPDATA%\Microsoft\Windows\INetCache\swell-player\updates\install.log`)
  says what it waited for and what it ended.

### Changed

- **zbus 6:** the media controls (MPRIS), the tray icon and the sleep
  inhibitor on Linux use zbus 6. In the Flatpak they need
  xdg-dbus-proxy 0.1.6 or newer on the host system; Ubuntu 24.04 and
  Debian 12 ship older ones.
- **Dependencies:** tokio 1.54.1 and lru 0.19. The macOS tray icon no
  longer uses the deprecated cocoa crate directly.

## [1.3.2] - 2026-10-24

### Fixed

- **Tray icon barely visible on KDE Plasma:** Plasma shows an app's
  symbolic icon in the tray and recolours it only through the colour
  scheme's stylesheet, which Swell's symbolic icon lacked: on a dark
  panel it stayed nearly black and showed only on hover. It now follows
  the panel's colours.
- **Two tray icons on Windows:** GLib (2.88 and later) adds a
  notification-area icon of its own the first time an application sends
  or withdraws a notification, and never removes it. Swell no longer
  uses GLib's notifications on Windows: track notifications come from
  its own tray icon, so they show while Run in Background is on. The
  icon also has a fixed identity, so one left by a Swell that ended
  abruptly is removed when Swell starts again, and it goes away when
  Windows ends the session.

## [1.3.1] - 2026-10-22

### Fixed

- **Empty "View All" pages:** TIDAL sends some of the home's "View All"
  pages (Personal Radio Stations, at times Based on Your Interests) with
  no items, so they said "Nothing Here Yet" although the home showed
  some. They now show what the home showed.

## [1.3.0] - 2026-10-21

### Added

- **Tray icon:** with Run in Background on, Swell shows an icon in the
  tray (a StatusNotifierItem on Linux: KDE Plasma, Xfce, waybar, GNOME
  with the AppIndicator extension…; the notification area on Windows;
  the menu bar on macOS) while it runs. A click brings the window back;
  its menu plays or pauses, skips and quits.

### Fixed

- **Memory growing with every track:** freed audio stayed in glibc's
  arenas, so memory rose track after track although nothing leaked; the
  allocator now keeps at most two arenas and gives freed memory back once
  a track's pipeline stops. Hi-Res streams buffer 15 s ahead instead of
  30 s (about 35 MB less per deck at 24-bit/192 kHz). The now-playing
  panel no longer loads the large cover of every track while it is
  closed, and large covers keep a budget of their own (16 MB) within the
  artwork cache, so the covers of earlier tracks no longer pile up. The
  lyrics and play-time caches are bounded. In tests, memory now stays
  flat over 60 tracks instead of climbing.

## [1.2.0] - 2026-10-18

### Added

- **Updates:** Swell looks for a new release shortly after it starts and
  then once a day (Preferences → General → Updates, which can turn it off
  or check at once). The Windows installer (installed for the current
  user, the default) and the macOS app download the new version, check it
  against the release's `SHA256SUMS` and install it when Swell quits, or
  at once with Restart. Elsewhere (Linux packages, Flatpak, the tarball,
  the portable zip, an all-users install) a notification links to the
  release. Each version is announced once per run, and nothing shows while
  Swell is up to date. Distributions can build with
  `SWELL_DISABLE_UPDATE_CHECK=1` to leave updates to themselves.

### Fixed

- **Volume jumping to full on a track change:** a new track could start at
  the sound server's volume (often 100 %) instead of yours, after a skip,
  at the end of an album, after an error, with Volume Normalisation on,
  or on a gapless switch to a track of another sample rate. Swell now
  gives the output its volume and mute again whenever it opens a new
  stream.
- A failing audio output no longer counts as a failing track: Swell builds
  the output again and retries once, then pauses instead of skipping
  through the queue.
- Resuming while the pause fades out shows Playing again, and a pause in
  the last moment of a track keeps the next one paused.
- The seek bar and the mini player follow seeks made from the desktop's
  media controls, and those controls get the right position after a
  restart or a new track.
- Keyboard shortcuts work on non-Latin layouts and with Caps Lock or
  Num Lock on; the volume shortcuts unmute; Space opens the player from
  the mini player.
- Plain lyrics scroll back to the top for each track; the previous
  track's notification is withdrawn when the next one starts with the
  window in front.
- Playing, shuffling or queueing a long playlist that contains videos no
  longer queues some tracks twice; an early token refresh that fails
  keeps the session; signing in with a code survives a short network
  failure; mix covers no longer show another mix's picture.
- Prefetching backs off after a failure instead of retrying every five
  seconds, and an expired stream's start is no longer read back into
  memory.

## [1.1.0] - 2026-10-18

### Added

- French, German, Italian, Brazilian Portuguese, Russian, Japanese, Korean
  and Simplified and Traditional Chinese in the Language preference, with
  each language's plural rules; TIDAL's content follows the chosen
  language. The desktop entry and the software-centre description are
  translated too, and the README is available in each language.
- **Instant skips:** the next five tracks are prefetched while playing
  (their streams and first seconds of audio), so skipping to one starts in
  a few tens of milliseconds instead of a second or more. Nothing is
  prefetched on metered connections.
- **Audio cache on disk:** those first seconds are kept in
  `~/.cache/swell-player/audio/` (at most 512 MB, `audio_cache_mb` in the
  settings file, and never when the disk is short of space), so they
  survive restarts: the first track of a session, or one played before,
  starts from disk. Memory keeps only the next track's. Preferences →
  Storage shows its size and can clear it; signing out deletes it. Only
  audio is stored, never stream URLs or tokens.
- **Crossfade** between tracks that end by themselves (Preferences →
  Playback, 1–12 s, off by default), with or without Volume Normalisation;
  the overlapping seconds are mixed, so not bit-perfect. Consecutive
  tracks of an album stay gapless.
- **Smooth Transitions:** short fades when pausing, seeking and changing
  track, so the sound never stops with a click (on by default).

## [1.0.0] - 2026-10-18

The first public release.

### Added

- **Browsing:** personalised Home feed that loads more as you scroll;
  Explore with editorial pages, genres, moods and decades; album, artist,
  playlist, mix and track/artist radio pages; "View all" pages; recently
  played.
- **Search** as you type, with filters (tracks, albums, artists,
  playlists), a top hit, recent searches and paging; pasted TIDAL links
  open the right page.
- **Collection:** favourite tracks (thousands, virtualised, with filter and
  sort), albums, artists, playlists and mixes; hearts on every kind of
  object from a single request for the whole library.
- **Playlists:** create, rename, describe, publish/unpublish and delete;
  add tracks from menus or by drag and drop onto the sidebar; remove and
  reorder tracks, with undo; edits guarded against concurrent changes.
- **Playback:** Hi-Res Lossless (FLAC up to 24-bit/192 kHz over MPEG-DASH)
  and Lossless; bit-perfect output to PipeWire with normalisation off and
  volume at 100 %; gapless playback; optional ReplayGain (track or album);
  choice of output device; a format popover that explains whether the
  output is bit-perfect.
- **Queue:** play next, add to queue (tracks, albums, playlists), jump,
  remove, reorder, clear with undo; shuffle, repeat all/one, history;
  queue and position restored at startup.
- **Resilience:** waits out network drops and resumes at the same
  position; reloads expired stream URLs; skips unavailable tracks.
- **Now playing:** docked panel (wide windows) or bottom sheet (narrow)
  with large artwork, quality details, synced lyrics that follow the song
  and seek on click, and the queue.
- **Desktop integration:** MPRIS (media keys, widgets, `playerctl`),
  track-change notifications, sleep timer, suspend inhibition while
  playing, running in the background, `tidal://` and `https://tidal.com/…`
  link handling.
- **Interface:** libadwaita design following the system light/dark style
  and accent colour; wide, regular and narrow layouts down to phone size;
  full keyboard control with a shortcuts dialog; accessible names on every
  control.
- **Sign-in:** browser (PKCE, up to Hi-Res) and device-code (up to
  Lossless) flows; tokens kept in the system keyring and refreshed ahead of
  expiry.
- **Performance:** stale-while-revalidate SQLite cache so the library opens
  instantly; lazy, budgeted artwork loading; no network or disk work on the
  main thread.
- **Languages:** English and Spanish, chosen from the system locale or in
  Preferences; TIDAL content requested in the same language.
- **Preferences:** streaming quality, normalisation, gapless, output
  device, background mode, notifications, session restore, cache usage and
  clearing, blocked artists.
- **Packages:** AUR (`swell-player`, `swell-player-git`), Flatpak bundle,
  `.deb`, `.rpm`, Windows and macOS builds, and `make install` for
  building from source.
- **Translations:** `po/swell.pot` template, generated by `po/update-pot`,
  and a [translation guide](docs/translating.md).

[Unreleased]: https://github.com/oxakromax/swell-player/compare/v1.3.3...HEAD
[1.3.3]: https://github.com/oxakromax/swell-player/compare/v1.3.2...v1.3.3
[1.3.2]: https://github.com/oxakromax/swell-player/compare/v1.3.1...v1.3.2
[1.3.1]: https://github.com/oxakromax/swell-player/compare/v1.3.0...v1.3.1
[1.3.0]: https://github.com/oxakromax/swell-player/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/oxakromax/swell-player/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/oxakromax/swell-player/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/oxakromax/swell-player/releases/tag/v1.0.0
