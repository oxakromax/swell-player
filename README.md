# Swell

A lightweight, native TIDAL client for Linux, written in Rust with
[gtk4-rs](https://gtk-rs.org), [libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/)
and [GStreamer](https://gstreamer.freedesktop.org/).

> **Status: proof of concept.** Swell exists to find out how light and fast a
> native client can be compared with the Python/PyGObject client
> [High Tide](https://github.com/Nokse22/high-tide). It signs in, lists your
> favourite tracks and plays them, including Hi-Res Lossless. Nothing more yet.

Swell is not affiliated with, endorsed by or supported by TIDAL. It talks to
TIDAL's unofficial v1 API, which can change without notice. You need a paid
TIDAL subscription.

## Building

Dependencies (Arch Linux package names):

```bash
sudo pacman -S --needed rust gtk4 libadwaita gstreamer gst-plugins-base gst-plugins-good
```

Then:

```bash
cargo run --release
```

## Signing in and audio quality

TIDAL does not let third-party apps register their own OAuth clients, so Swell,
like every open-source TIDAL client, signs in as one of TIDAL's own apps. The
login flow you pick decides which app, and that caps the audio quality:

| Login | How it works | Best quality |
|---|---|---|
| **Browser (PKCE)** | Swell opens TIDAL's login page in your browser. After you sign in, TIDAL lands on a page that says "Oops"; copy that page's address back into Swell. | **Hi-Res Lossless**: FLAC up to 24-bit/192 kHz, delivered as MPEG-DASH |
| **Device code** | Swell shows a short code; you approve it at `link.tidal.com` from any device. | **Lossless**: FLAC 16-bit/44.1 kHz (CD quality) |

Use the browser login if you want Hi-Res. The device code is handy when you
cannot copy a URL, for example on a remote session.

The access and refresh tokens are stored in your desktop keyring through the
Secret Service API (KWallet, GNOME Keyring, KeePassXC…, or the secret portal
inside Flatpak) using [oo7](https://crates.io/crates/oo7). They are never
written to disk in plain text. Signing out deletes them. When TIDAL rejects an
expired access token, the client refreshes it and stores the new one.

## Design

- **No network on the main thread.** GTK owns the main thread; all HTTP and
  keyring I/O runs on a small Tokio runtime (two worker threads) and is awaited
  from the GLib main loop.
- **Few, warm connections.** One `reqwest` client serves the API and the
  images. Both TIDAL hosts speak HTTP/2, so concurrent requests share one
  connection per host, and Swell opens both connections at startup while it
  reads the keyring. Names are resolved with
  [hickory](https://crates.io/crates/hickory-resolver) rather than glibc: on
  networks where one of glibc's parallel A/AAAA answers gets lost, every
  lookup stalls for 5 s, which used to delay the first list by that much.
- **Favourites arrive page by page.** The first page of 100 tracks is shown as
  soon as it arrives; the rest is fetched with at most four requests in
  flight and appended in order. (TIDAL's total counts tracks that are no
  longer available and leaves them out of the pages, so a page can hold fewer
  than 100.)
- **Virtualised list.** Tracks live in a `gio::ListStore` of compact records
  shown by a `gtk::ListView` that is the direct child of its
  `ScrolledWindow`, so GTK only keeps widgets for the rows around the
  viewport (about 200), not one per track.
- **Covers.** 80×80 JPEGs, at most four downloads at a time, cached on disk
  under `~/.cache/swell-player/covers/` and kept decoded in a 400-entry LRU
  (under 10 MB). The pending request nearest the middle of the viewport goes
  first, newest first among equals; plain LIFO is not enough because the list
  binds ~200 rows up front and the last of them is far off screen. Requests
  from rows that scroll away before a download starts are dropped.
- **API client:** [tidalrs](https://crates.io/crates/tidalrs) (MIT), built
  without its playback features. It was chosen over
  [tidlers](https://crates.io/crates/tidlers) (whose 0.5.0 favourites call
  sends `ofset` instead of `offset`, so it cannot paginate) and
  [rstidal](https://crates.io/crates/rstidal) (unmaintained since 2020). It
  covers PKCE and device-code login, automatic token refresh with a callback,
  paginated favourites and the raw playback manifest.

## License

Swell is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. See [COPYING](COPYING).
