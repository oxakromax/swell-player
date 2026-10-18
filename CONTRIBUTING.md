# Contributing to Swell

Thanks for helping! Bug reports, fixes, translations and ideas are all
welcome. For anything bigger than a small fix, please open an issue first
so we can agree on the approach before you spend time on it.

By contributing you agree that your work is released under the project's
license, GPL-3.0-or-later (see [COPYING](COPYING)), and you agree to follow
the [Code of Conduct](CODE_OF_CONDUCT.md).

## Building

Install the dependencies listed in the README's
[From source](README.md#from-source) section (Rust ≥ 1.96, GTK 4,
libadwaita, GStreamer with the base and good plugins, SQLite and
`glib-compile-resources`), then:

```bash
cargo run --release
```

A debug build works too, but list scrolling and image decoding are much
slower in it. `make` builds exactly what packages ship
(`cargo build --release --locked`), and `make check` validates the desktop
file and metainfo.

You need a TIDAL subscription to sign in. The session is stored in your
keyring and shared by every build you run.

## Before sending a change

```bash
cargo fmt --all
cargo clippy --release --all-targets   # no new warnings
cargo test --release
po/update-pot                          # if you added or changed UI strings
```

The tests include `every_string_is_translated` (in `src/i18n.rs`), which
fails when a `tr("…")` string has no Spanish translation: add one to
`po/es.po` (or ask in the pull request and someone will).

## Code guidelines

[docs/architecture.md](docs/architecture.md) describes the modules. In
short:

- **Never block the GTK main thread.** Network, disk, keyring and image
  decoding go through `runtime::spawn(async move { … }).await` from a
  `glib::spawn_future_local` task.
- Lists that can be long are `gtk::ListView` / `gtk::GridView` in a
  `ScrolledWindow`, with fixed row heights and artwork from the cover
  loader.
- Hold `Rc<Ctx>` weakly from closures stored in widgets, and cancel async
  work when a page is destroyed.
- Every icon-only button has a tooltip and an accessible label.
- Every user-visible string goes through `tr("…")` or
  `trn("…", "…", n)`, with named placeholders (`{name}`) replaced after
  translating.
- Match the surrounding style: doc comments on modules and non-obvious
  items, no noise comments. Prefer no new crates; if one is needed, use its
  latest stable version.
- The interface follows [docs/design.md](docs/design.md) (in Spanish).
- Don't copy code from other TIDAL clients or libraries (High Tide,
  tidalapi…); knowledge of the protocol is fine.

## Debugging aids

Run with `RUST_LOG=swell_player=debug` for detailed logs (the default level
is `warn`).

These environment variables are inert unless set (`src/ui/debug.rs` has
the full list). Screenshot runs don't change your settings, and
`SWELL_PLAY` doesn't overwrite your saved queue.

| Variable | Effect |
|---|---|
| `SWELL_ROUTE=<route>` | Opens a page once signed in: `home`, `explore`, `search:<query>`, `tracks`, `albums`, `artists`, `playlists`, `mixes`, `history`, `album:<id>`, `artist:<id>`, `playlist:<uuid>`, `mix:<id>`, `radio:<track id>`, `page:<api path>`, `more-data:`/`more-view:`/`more-page:<path>`. |
| `SWELL_SECTION=home\|explore\|search\|collection` | Shows that section first. |
| `SWELL_SEARCH_FILTER=tracks\|albums\|artists\|playlists` | Starts Search with that filter. |
| `SWELL_PLAY=<track id>[@<seconds>]` | Plays a track at startup (`restore` restores the saved queue). |
| `SWELL_PANEL=playing\|lyrics\|queue` | Opens the now-playing panel on that tab. |
| `SWELL_DIALOG=preferences\|shortcuts\|about` | Opens a dialog; `SWELL_PREFERENCES_PAGE=playback\|general\|account` picks the Preferences page. |
| `SWELL_ACTIONS=win.a,player.b('x'),…` | Activates actions one after another, 700 ms apart. |
| `SWELL_LOGIN=1` | Shows the sign-in view (the stored session is kept). |
| `SWELL_SCREENSHOT=<file.png>` | Renders the window to a PNG after `SWELL_SCREENSHOT_DELAY_MS` (default 5000) and quits. `SWELL_SCREENSHOT_SIZES=1400x900,400x800` takes one shot per size (`file-WxH.png`). |
| `SWELL_DEBUG_STREAM_FAIL=<n>[@<skip>]` | Makes `n` stream lookups fail as if the network were down. |
| `SWELL_DEBUG_ENGINE_FAIL=<at>[@<status>]` | Simulates a playback network error (see `src/playback/mod.rs`). |
| `SWELL_DEBUG_SKIP=<seconds>[x<count>]` | Skips to the next track every `seconds` of playback; with debug logs, "first audio … ms" says how fast each start was. |
| `SWELL_NO_PROXY=1` | Streams straight from TIDAL, without the local proxy (and so without prefetched audio). |
| `SWELL_PREFETCH_BUDGET_MB=<n>` | Caps the prefetched audio at `n` MB (at most 64), to watch eviction. |

Useful ids: album `529955456`, artist `17275`, track `529955457`.

### Headless screenshots

You can render Swell without touching your desktop, for example to check
a layout at several sizes or to update `docs/screenshots/`:

```bash
kwin_wayland --virtual --socket wayland-shots --width 1400 --height 900 --no-lockscreen &
KWIN=$!

WAYLAND_DISPLAY=wayland-shots LANG=C.UTF-8 \
SWELL_ROUTE=album:529955456 SWELL_PANEL=playing \
SWELL_SCREENSHOT=/tmp/album.png SWELL_SCREENSHOT_SIZES=1400x900,1000x760,400x800 \
SWELL_SCREENSHOT_DELAY_MS=6000 \
  ./target/release/swell-player

kill $KWIN
```

Any headless Wayland compositor works (`weston --backend=headless`,
`cage`, …). `ADW_DEBUG_COLOR_SCHEME=prefer-light` or `prefer-dark` forces
the style. Add `SWELL_PLAY=529955457@45` to show the player bar and synced
lyrics (it plays audio: lower the volume first, or point
`XDG_CONFIG_HOME` at a scratch directory with a quiet `settings.json`).
Pass the README screenshots through a lossless PNG optimiser (`oxipng -o
max`) and keep each under 500 KB.

## Commits and pull requests

- One logical change per commit. The subject is a short sentence in the
  imperative, without a prefix or a trailing period ("Keep list rows when
  filtering", "Play through the output device chosen in Preferences"); the
  body, if any, says why.
- Keep pull requests focused, and describe how you tested them; for
  interface changes, include a screenshot.
- Update [CHANGELOG.md](CHANGELOG.md) under **Unreleased** for changes
  users will notice.

## Translations

See [docs/translating.md](docs/translating.md). Translators don't need to
build Swell to start: copy `po/swell.pot`, translate it, and open a pull
request.

## Reporting bugs and security issues

Use the [issue tracker](https://github.com/oxakromax/swell-player/issues)
for bugs and feature requests. Report security problems privately as
described in [SECURITY.md](SECURITY.md).
