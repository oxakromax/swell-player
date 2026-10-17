# Swell

A fast, lightweight, native TIDAL client for Linux, written in Rust with
[gtk4-rs](https://gtk-rs.org), [libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/)
and [GStreamer](https://gstreamer.freedesktop.org/). Bit-perfect Hi-Res
playback, a library that opens instantly from a local cache, and a
responsive interface that works from phone-sized windows to wide desktops.

Swell is not affiliated with, endorsed by or supported by TIDAL. It talks to
TIDAL's unofficial v1/v2 APIs, which can change without notice. You need a
paid TIDAL subscription.

## Features

**Browse**
- Home: your personalised TIDAL feed (shortcuts, mixes, recommendations,
  recently played), loading more as you scroll.
- Explore: editorial pages, genres, moods and decades.
- Search as you type, with filters (tracks, albums, artists, playlists), a
  top hit, recent searches and paging; pasting a TIDAL link opens it.
- Album pages (discs, credits, review, more by the artist), artist pages
  (top tracks, albums, EPs and singles, appearances, similar artists, bio,
  radio), playlist pages, mixes and track/artist radio, "View all" pages.
- `tidal://` and `https://tidal.com/...` links open in Swell.

**Your collection**
- Favourite tracks (thousands, virtualised) with instant filter and sort,
  favourite albums, artists, playlists and mixes, recently played.
- Hearts everywhere (tracks, albums, artists, playlists, mixes), from one
  request for the whole library.
- Create, rename, describe, publish/unpublish and delete playlists; add
  tracks (menu or drag and drop onto a sidebar playlist), remove and reorder
  them, with undo.

**Playback**
- Hi-Res Lossless (FLAC up to 24-bit/192 kHz), bit-perfect: with
  normalisation off and volume at 100 %, samples reach PipeWire untouched
  and the device runs at the track's own rate (see below).
- Gapless playback, ReplayGain normalisation (track or album) as an option,
  choice of output device.
- A real queue: play next, add to queue (tracks, albums, playlists), jump,
  remove, reorder, clear with undo; shuffle and repeat (all/one); history.
- Survives network drops (waits and resumes at the same position) and
  expired stream URLs.
- Synced lyrics that follow the song and seek on click.
- Restores your queue and position at startup; sleep timer; desktop
  notifications; keeps the system awake while playing.
- MPRIS: media keys, desktop widgets and `playerctl` work.

**Interface**
- libadwaita design that follows the system's light/dark style and accent
  colour; three layouts (wide with docked now-playing panel, regular,
  narrow with bottom sheet and mini player).
- Full keyboard control (Space, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…; Ctrl+? lists
  them all) and accessible names on every control.
- English and Spanish; TIDAL content is requested in the same language.

## Building

Dependencies (Arch Linux package names):

```bash
sudo pacman -S --needed rust gtk4 libadwaita gstreamer gst-plugins-base gst-plugins-good
```

`gst-plugins-good` provides the DASH demuxer (`dashdemux2`) used for Hi-Res streams.
`glib-compile-resources` (part of GLib) is needed at build time.

```bash
cargo run --release
```

The binary runs from anywhere (icons and translations are embedded). To get
a launcher entry and the `tidal://` handler, install
`data/io.github.oxakromax.SwellPlayer.desktop` and the icon in
`data/icons/` the usual way.

## Signing in and audio quality

TIDAL does not let third-party apps register their own OAuth clients, so
Swell, like every open-source TIDAL client, signs in as one of TIDAL's own
apps. The login flow you pick decides which, and that caps the quality:

| Login | How it works | Best quality |
|---|---|---|
| **Browser (PKCE)** | Swell opens TIDAL's login page in your browser. After you sign in, TIDAL lands on a page that says "Oops"; copy that page's address back into Swell. | **Hi-Res Lossless**: FLAC up to 24-bit/192 kHz, as MPEG-DASH |
| **Device code** | Swell shows a short code; you approve it at `link.tidal.com` from any device. | **Lossless**: FLAC 16-bit/44.1 kHz |

Tokens live in your desktop keyring (Secret Service: KWallet, GNOME Keyring,
KeePassXC…, or the secret portal in Flatpak) via
[oo7](https://crates.io/crates/oo7), never on disk in plain text. They are
refreshed shortly before they expire. Only a refresh token TIDAL rejects
(`invalid_grant`) signs you out; network trouble never does. Signing out
deletes the tokens, the cache, the saved queue and search history.

## Bit depth and sample rate

Swell keeps the stream's format all the way to PipeWire (a 24-bit track
reaches it as 24-bit). PipeWire decides what the sound card gets: by default
it runs at one rate (often 48 kHz) and resamples anything else. To let it
switch to each track's rate, allow the common rates, e.g. in
`~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

and restart PipeWire (`systemctl --user restart pipewire`). `pw-top` shows
what reaches the device. The format badge in the player bar opens a popover
that says whether the output is bit-perfect and, if not, why (normalisation
or volume below 100 %).

## Design

- **Own TIDAL client** (`src/tidal/`): v1 and v2 endpoints over one HTTP/2
  connection per host, lenient models (a malformed item drops only itself),
  token refresh shared by concurrent requests, retries only for idempotent
  requests, playlist edits guarded by ETags.
- **No network or disk on the main thread.** GTK owns it; HTTP, SQLite,
  keyring and image decoding run on a small Tokio runtime.
- **Stale-while-revalidate cache.** Pages and your library are drawn from
  SQLite (`~/.cache/swell-player/library.sqlite3`) at once and refreshed in
  the background; favourites are validated with one request, and tracks
  liked elsewhere are prepended without downloading the list again. Old
  answers and least-used artwork are pruned automatically.
- **Virtualised lists and lazy artwork.** Long lists are `gtk::ListView` /
  `GridView`; covers load only near the visible area, at the size shown, at
  most four at a time, nearest the middle of the viewport first, with a
  64 MB budget of decoded textures.
- **DNS without stalls.** Names are resolved with
  [hickory](https://crates.io/crates/hickory-resolver): on some networks
  glibc's parallel A/AAAA lookups stall for 5 s.
- **Playback** (`src/playback/`): GStreamer `playbin3`; Hi-Res DASH
  manifests are written to a private per-process directory under
  `$XDG_RUNTIME_DIR` and streamed by `dashdemux2`; the next track's stream is
  prepared ahead and handed over on `about-to-finish` for gapless playback.

## Measurements

MEASUREMENTS_PLACEHOLDER

## Development

`docs/` holds the feature scope (`features.md`), the interface specification
(`design.md`) and notes for contributors (`contributing-agents.md`), including
environment variables that open a page, play a track or render the window to
a PNG without a display (`src/ui/debug.rs`).

```bash
cargo test --release
cargo clippy --release
```

## License

Swell is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. See [COPYING](COPYING).
