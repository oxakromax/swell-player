<div align="center">

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**A fast, native TIDAL client with bit-perfect Hi-Res playback.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell showing an album, with the now-playing panel docked on the right](docs/screenshots/album.png)

</div>

Swell is written in Rust with [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) and
[GStreamer](https://gstreamer.freedesktop.org/). Your library opens
instantly from a local cache, Hi-Res FLAC reaches the sound server
untouched, and the interface adapts from phone-sized windows to wide
desktops, using a fraction of the memory of other clients
([measurements](#measurements)).

> [!NOTE]
> Swell is not affiliated with, endorsed by or supported by TIDAL. It uses
> TIDAL's unofficial APIs, which can change without notice, and needs a paid
> TIDAL subscription.

## Contents

- [Features](#features)
- [Screenshots](#screenshots)
- [Installation](#installation)
- [Signing in and audio quality](#signing-in-and-audio-quality)
- [Bit-perfect playback](#bit-perfect-playback)
- [Languages](#languages)
- [Privacy](#privacy)
- [Measurements](#measurements)
- [Development](#development)
- [License](#license)

## Features

**Browse**
- Home: your personalised TIDAL feed (shortcuts, mixes, recommendations,
  recently played), loading more as you scroll. Explore: editorial pages,
  genres, moods and decades.
- Search as you type, with filters (tracks, albums, artists, playlists), a
  top hit, recent searches and paging; pasting a TIDAL link opens it.
- Album, artist, playlist, mix and radio pages, with credits, reviews, bios,
  similar artists and "View all" pages.
- `tidal://` and `https://tidal.com/…` links open in Swell.

**Your collection**
- Thousands of favourite tracks, virtualised, with instant filter and sort;
  favourite albums, artists, playlists and mixes; recently played.
- Hearts on everything (tracks, albums, artists, playlists, mixes), from a
  single request for the whole library.
- Create, rename, describe, publish and delete playlists; add tracks from a
  menu or by dragging them onto a sidebar playlist; remove and reorder them,
  with undo.

**Playback**
- Hi-Res Lossless (FLAC up to 24-bit/192 kHz), [bit-perfect](#bit-perfect-playback).
- Gapless playback, optional ReplayGain (track or album), choice of output
  device.
- A real queue: play next, add to queue, jump, remove, reorder, clear with
  undo; shuffle, repeat all/one, history.
- Survives network drops and expired stream URLs, resuming at the same
  position.
- Synced lyrics that follow the song and seek on click.
- Restores your queue and position at startup; sleep timer; desktop
  notifications; keeps the system awake while playing.
- MPRIS on Linux: media keys, desktop widgets and `playerctl` work.

**Interface**
- libadwaita design that follows the system's light/dark style and accent
  colour, with three layouts: wide with a docked now-playing panel, regular,
  and narrow with a bottom sheet and mini player.
- Full keyboard control (Space, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…; Ctrl+? lists
  every shortcut) and accessible names on every control.
- English and Spanish.

## Screenshots

| Home | Search |
|---|---|
| ![Home feed with shortcuts, mixes and recently played](docs/screenshots/home.png) | ![Search results for "yoasobi" with a top hit, tracks and artists](docs/screenshots/search.png) |
| **Synced lyrics** | **Light style** |
| ![Now-playing panel showing synced lyrics](docs/screenshots/now-playing.png) | ![The album page in the light style](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="Narrow layout with the mini player and the bottom navigation bar">
</p>

## Installation

Packages for each release are on the
[Releases page](https://github.com/oxakromax/swell-player/releases).
Swell needs GTK ≥ 4.18, libadwaita ≥ 1.7 and GStreamer with the base and
good plugins; the packages pull these in.

### Arch Linux (AUR)

```bash
yay -S swell-player        # latest release
yay -S swell-player-git    # development version
```

### Flatpak

Download the `.flatpak` bundle from the Releases page and install it (it
uses the GNOME runtime from Flathub). A Flathub listing is planned.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian and Ubuntu

Requires Debian 13 (trixie), Ubuntu 25.04 or newer (GTK ≥ 4.18 and
libadwaita ≥ 1.7). Download the `.deb` from the Releases page:

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Download the `.rpm` from the Releases page:

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Download the installer (or the portable `.zip`) from the Releases page.
The builds are not code-signed yet, so SmartScreen may warn about an
unrecognised app: choose **More info → Run anyway**.

### macOS

Download the `.dmg` from the Releases page and drag Swell to
Applications. The app is not notarised yet, so the first time,
right-click (or Control-click) it and choose **Open**; on macOS 15 and
later, open it once, then allow it in **System Settings → Privacy &
Security → Open Anyway**.

Windows and macOS builds are newer than the Linux one: MPRIS and the
bit-perfect notes below are Linux-specific.

### From source

You need Rust ≥ 1.88 ([rustup](https://rustup.rs) if your distribution's is
older), GTK 4, libadwaita, GStreamer, SQLite and GLib's
`glib-compile-resources`.

<details>
<summary>Build dependencies per distribution</summary>

```bash
# Arch Linux
sudo pacman -S --needed rust pkgconf gtk4 libadwaita sqlite \
    gstreamer gst-plugins-base gst-plugins-good

# Debian / Ubuntu
sudo apt install cargo pkg-config libgtk-4-dev libadwaita-1-dev libsqlite3-dev \
    libglib2.0-dev-bin libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-good

# Fedora
sudo dnf install cargo pkgconf-pkg-config gtk4-devel libadwaita-devel sqlite-devel \
    glib2-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good
```

`gst-plugins-good` provides the DASH demuxer (`dashdemux2`) used for Hi-Res
streams.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` stages an install for packaging, and
`sudo make uninstall` removes it. The binary also runs straight from
`target/release/swell-player`, since icons and translations are embedded.

## Signing in and audio quality

TIDAL does not let third-party apps register their own OAuth clients, so
Swell, like every open-source TIDAL client, signs in as one of TIDAL's own
apps. The login flow you pick decides which, and that caps the quality:

| Login | How it works | Best quality |
|---|---|---|
| **Browser** (recommended) | Swell opens TIDAL's login page in your browser. After you sign in, TIDAL lands on a page that says "Oops"; copy that page's address back into Swell. | **Hi-Res Lossless**: FLAC up to 24-bit/192 kHz |
| **Device code** | Swell shows a short code; approve it at `link.tidal.com` from any device. | **Lossless**: FLAC 16-bit/44.1 kHz |

Choose the streaming quality in **Preferences → Playback**. The format badge
in the player bar shows what is actually being delivered.

## Bit-perfect playback

With normalisation off and volume at 100 %, Swell keeps the stream's format
all the way to PipeWire: a 24-bit/96 kHz track reaches it as 24-bit at
96 kHz. The format badge in the player bar opens a popover that says
whether the output is bit-perfect and, if not, why.

PipeWire then decides what the sound card gets. By default it runs at one
rate (often 48 kHz) and resamples everything else. To let it follow each
track's rate, allow the common rates, for example in
`~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

then restart PipeWire (`systemctl --user restart pipewire`). `pw-top` shows
what reaches the device.

## Languages

Swell speaks **English** and **Spanish**. It follows the system language
automatically (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, like gettext),
and you can override it in **Preferences**. TIDAL's own content (editorial
texts, page titles) is requested in the same language, so a screen never
mixes two.

Want Swell in your language? See [docs/translating.md](docs/translating.md).

## Privacy

- **Tokens** live in your system keyring (on Linux, the Secret Service:
  GNOME Keyring, KWallet, KeePassXC…, or the secret portal in Flatpak),
  never on disk in plain text. They are refreshed shortly before they
  expire, and network trouble never signs you out.
- **Cached on your computer:** TIDAL's answers and your library
  (`~/.cache/swell-player/library.sqlite3`), artwork, your settings, the
  saved queue and recent searches. Preferences can clear the cache; signing
  out deletes the tokens, the cache, the saved queue and the search history.
- **No telemetry.** Swell connects only to TIDAL's servers, for what you
  ask it to show or play. It has no analytics, crash reporting or other
  third-party services.

## Measurements

Swell against [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject) on the same machine, both in a headless KWin session
(`kwin_wayland --virtual`, 1280×860) with a saved session. Medians of three
runs; memory read from `/proc` (RssAnon is the app's own heap; RSS also
counts shared libraries and about 250 MB of GPU driver mappings that any
GTK 4 app has here). Idle figures are averaged over 10 s with nothing
playing.

| | Swell | High Tide |
|---|---|---|
| Home on screen: RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 1,295 favourite tracks on screen, from the cache | **0.42 s** after launch (in sync with TIDAL at 0.50 s) | — ¹ |
| … without Swell's cache | first 100 at **0.59 s**, all at 1.2–1.9 s | — ¹ |
| Favourites view: RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| Idle CPU / main-thread wakeups | ~0 % / 1.1 per s | ~0 % / 0.1 per s |
| Threads | 15 | 19 |
| Hi-Res track chosen → audio | **0.67 s** (24-bit/96 kHz, reaches PipeWire as S24_32 at 96 kHz) | — |

¹ High Tide does not cache favourites and has no way to open its favourites
list directly; with a test harness, Swell's proof of concept measured
13.3 s for High Tide to show the same list, against 0.3 s for Swell.

Machine: Arch Linux, KDE Plasma 6 (Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, release build.

## Development

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) covers building, testing, the debugging
environment variables (open a page, play a track, or render the window to a
PNG without a display) and how to send changes.
[docs/architecture.md](docs/architecture.md) explains how the code is
organised; [docs/features.md](docs/features.md) is the feature scope and
[docs/design.md](docs/design.md) the interface specification (in Spanish).
Release notes are in [CHANGELOG.md](CHANGELOG.md).

Found a security problem? Please report it privately: see
[SECURITY.md](SECURITY.md).

## License

Swell is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. See [COPYING](COPYING).

TIDAL is a trademark of its owner, used here only to say what Swell works
with.
