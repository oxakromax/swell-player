# Packaging Swell for Linux

Everything here installs the same files, through the top-level `Makefile`
or the equivalent `Cargo.toml` metadata:

| File | Installed to |
|---|---|
| `swell-player` (the binary; icons and translations are embedded) | `$PREFIX/bin/` |
| `data/io.github.oxakromax.SwellPlayer.desktop` (also the `tidal://` handler) | `$PREFIX/share/applications/` |
| `data/io.github.oxakromax.SwellPlayer.metainfo.xml` | `$PREFIX/share/metainfo/` |
| `data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg` | `$PREFIX/share/icons/hicolor/scalable/apps/` |
| `data/icons/symbolic/apps/io.github.oxakromax.SwellPlayer-symbolic.svg` | `$PREFIX/share/icons/hicolor/symbolic/apps/` |
| `COPYING` | `$PREFIX/share/licenses/swell-player/` |
| `README.md`, `CHANGELOG.md` | `$PREFIX/share/doc/swell-player/` |

## Requirements

Build: Rust 1.96 or newer, a C compiler, `pkg-config`, `glib-compile-resources`
(GLib's development tools) and the development files of GTK 4, libadwaita,
GStreamer and SQLite.

Run:

- **GTK ≥ 4.20 and libadwaita ≥ 1.8** with the default features, or
  **GTK ≥ 4.18 and libadwaita ≥ 1.7** with `--no-default-features` (the
  `adw-1-8` feature in `Cargo.toml` decides).
- **GStreamer ≥ 1.24** with the *base* plugins (`playbin3`, `audioconvert`,
  `volume`) and the *good* plugins (`dashdemux2` for Hi-Res DASH streams,
  FLAC, MP4 and the HTTP source). The AAC qualities (Low/High) also need an
  AAC decoder, e.g. gst-libav.
- A Secret Service keyring (GNOME Keyring, KWallet, KeePassXC…) to stay
  signed in, or the Secret portal inside Flatpak.
- PipeWire is recommended for bit-perfect output but not required.

Distributions that meet this out of the box: Arch Linux, Fedora 43+,
Ubuntu 25.10+, Debian 14 "forky"/sid, openSUSE Tumbleweed. Anything older
can use the Flatpak.

## From source (`make`)

```bash
make                         # cargo build --release --locked
make check                   # desktop-file-validate + appstreamcli validate --no-net
sudo make install            # PREFIX=/usr/local by default
sudo make uninstall
```

For packages: `make install PREFIX=/usr DESTDIR="$pkgdir"`. `install` never
builds, so run `make` (or `cargo build --release`) first as a normal user.
Variables: `CARGO`, `CARGOFLAGS` (e.g. `CARGOFLAGS=--no-default-features`),
`CARGO_TARGET_DIR`, `PREFIX`, `DESTDIR`, `BINDIR`, `DATADIR`.

## Update checks

Swell looks for new releases on GitHub
(`api.github.com/repos/oxakromax/swell-player/releases/latest`) shortly
after it starts, then once a day, and from Preferences → General →
Updates. The Windows installer and the macOS app then install the new
version themselves (`src/update/`); every other install only shows a
notification that links to the release.

A distribution that delivers Swell's updates itself should build without
the checks:

```bash
SWELL_DISABLE_UPDATE_CHECK=1 make       # or SWELL_DISABLE_UPDATE_CHECK=1 cargo build --release --locked
```

The variable is read when the binary is compiled (any value other than
empty or `0`); such a build never contacts GitHub and has no Updates group
in Preferences. `flatpak/flathub-manifest.sh` sets it for Flathub. Builds
run from a Cargo target directory don't check either, unless
`SWELL_UPDATE_CHECK=1` is set when they run (to test the checks).

## Arch Linux (AUR)

Two packages live in `aur/`:

- `aur/swell-player`: the release, built from the GitHub tag tarball.
- `aur/swell-player-git`: the latest commit, version from `git describe`.

Build and install one locally:

```bash
cd packaging/aur/swell-player-git
makepkg -si
```

### Publishing on the AUR

Publishing needs the maintainer's AUR account with an SSH key registered at
<https://aur.archlinux.org/account> (the key is not in this repository).

1. Tag and push the release (`git tag v1.0.0 && git push origin v1.0.0`).
2. Fill in the checksum: `packaging/aur/update.sh` downloads
   `https://github.com/oxakromax/swell-player/archive/refs/tags/v1.0.0.tar.gz`,
   writes its sha256 into `aur/swell-player/PKGBUILD` (replacing the
   all-zeros placeholder) and regenerates both `.SRCINFO` files. Commit the
   result.
3. Check that it builds in a clean chroot (`pkgctl build` from `devtools`,
   or at least `makepkg -f` and `namcap *.pkg.tar.zst`).
4. Push each package to its AUR repository (the first push creates it):

   ```bash
   git clone ssh://aur@aur.archlinux.org/swell-player.git aur-swell-player
   cp packaging/aur/swell-player/{PKGBUILD,.SRCINFO} aur-swell-player/
   cd aur-swell-player
   git add PKGBUILD .SRCINFO
   git commit -m "Update to 1.0.0"
   git push
   ```

   Same for `ssh://aur@aur.archlinux.org/swell-player-git.git` with
   `packaging/aur/swell-player-git/`. The `-git` package only needs a new
   push when its PKGBUILD changes, not for every commit.

For later releases: bump the version in `Cargo.toml`, tag, run
`packaging/aur/update.sh` (it also resets `pkgrel` to 1) and push again. To
rebuild the same version (packaging fix only), increase `pkgrel` by hand
and regenerate `.SRCINFO` with `makepkg --printsrcinfo > .SRCINFO`.

## Flatpak

`flatpak/io.github.oxakromax.SwellPlayer.yml` builds against the GNOME 49
runtime with the Rust SDK extension. Crates are vendored from
`flatpak/cargo-sources.json`, so the build runs offline.

```bash
flatpak install --user flathub org.flatpak.Builder org.gnome.Sdk//49 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak run org.flatpak.Builder --user --install --force-clean \
    build-dir packaging/flatpak/io.github.oxakromax.SwellPlayer.yml
flatpak run io.github.oxakromax.SwellPlayer
```

A single-file bundle to share: build with `--repo=repo` and then
`flatpak build-bundle repo swell.flatpak io.github.oxakromax.SwellPlayer`.

**Whenever `Cargo.lock` changes**, run `packaging/flatpak/update-cargo-sources.sh`
(needs `uv`, or python3 with aiohttp, PyYAML and tomlkit) and commit the new
`cargo-sources.json`. That includes Dependabot's updates: CI runs
`update-cargo-sources.sh --check` (offline) and fails while the two differ.

Permissions are kept to display, GPU, network, audio (PulseAudio protocol,
which PipeWire serves) and the MPRIS bus name; the keyring (Secret portal),
notifications, "keep awake" and opening links go through portals. The
manifest explains each one.

### Submitting to Flathub

Flathub builds from its own repository (`flathub/io.github.oxakromax.SwellPlayer`),
created from a pull request. See <https://docs.flathub.org/docs/for-app-authors/submission>.

1. Make sure the release is tagged on GitHub, the screenshots in the
   metainfo are reachable at the tag
   (`https://raw.githubusercontent.com/oxakromax/swell-player/v1.0.0/docs/screenshots/*.png`)
   and the metainfo has a `<release>` for that version.
2. **App ID and repository name.** Flathub checks that an
   `io.github.<user>.<name>` ID matches `https://github.com/<user>/<name>`.
   For `io.github.oxakromax.SwellPlayer` that is
   `github.com/oxakromax/SwellPlayer` (GitHub names are case-insensitive), so
   the repository must be called `SwellPlayer` (the linter lowercases it),
   or the reviewers must accept an exception; a repo named `swell-player`
   fails the `appid-url-not-reachable` check (the ID matching that name
   would be `io.github.oxakromax.swell_player`, since Flathub writes the
   dash as an underscore). Settle this before creating the repository:
   changing the app ID later is painful (it keys the keyring entry,
   settings, cache and MPRIS name).
3. Generate the Flathub manifest, which points at the tag instead of the
   local checkout:

   ```bash
   git fetch --tags
   packaging/flatpak/flathub-manifest.sh 1.0.0 > /tmp/io.github.oxakromax.SwellPlayer.yml
   ```

4. Fork <https://github.com/flathub/flathub>, clone the fork with
   `--branch=new-pr`, create a branch, add that manifest and
   `packaging/flatpak/cargo-sources.json` at the top level, and open a pull
   request **against the `new-pr` branch** titled
   `Add io.github.oxakromax.SwellPlayer`.
5. Before (and after) opening it, run the same checks Flathub runs:

   ```bash
   flatpak run --command=flathub-build org.flatpak.Builder --install io.github.oxakromax.SwellPlayer.yml
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest io.github.oxakromax.SwellPlayer.yml
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
   ```

6. Answer the review; comment `bot, build` on the PR for a test build. Once
   merged, you get write access to the new
   `flathub/io.github.oxakromax.SwellPlayer` repository; future updates are
   PRs there (manifest tag/commit + regenerated `cargo-sources.json`), and
   `x-checker-data` lets Flathub's bot open them for new tags. Log in at
   <https://flathub.org> with GitHub to verify the app.

Flathub's linter may suggest a newer GNOME runtime; moving to one only needs
`runtime-version` changed (newer GTK/libadwaita keep the feature levels).

## Debian / Ubuntu (.deb) and Fedora (.rpm)

`Cargo.toml` carries `[package.metadata.deb]` for
[cargo-deb](https://crates.io/crates/cargo-deb) and
`[package.metadata.generate-rpm]` for
[cargo-generate-rpm](https://crates.io/crates/cargo-generate-rpm).

Natively, on the target distribution:

```bash
# Debian 13+ / Ubuntu 25.10+, with Rust ≥ 1.96 from rustup
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
    libgstreamer1.0-dev libsqlite3-dev libglib2.0-dev-bin
cargo install cargo-deb
cargo build --release --locked --no-default-features
cargo deb --no-build            # target/debian/swell-player_1.0.0-1_amd64.deb

# Fedora 43+
sudo dnf install gcc pkgconf-pkg-config rpm-build gtk4-devel libadwaita-devel \
    gstreamer1-devel sqlite-devel glib2-devel
cargo install cargo-generate-rpm
cargo build --release --locked
cargo generate-rpm              # target/generate-rpm/swell-player-1.0.0-1.x86_64.rpm
```

Or in a clean container (Docker or `CONTAINER=podman`), from the committed
tree:

```bash
packaging/linux/build-in-container.sh deb     # ubuntu:25.10 by default
packaging/linux/build-in-container.sh rpm     # fedora:43 by default
ls target/packages/
```

The release `.deb` is built with `--no-default-features` on Debian 13, so it
runs on GTK 4.18 and libadwaita 1.7: it gets its library dependencies from
`dpkg-shlibdeps`, states `libgtk-4-1 (>= 4.18)`, `libadwaita-1-0 (>= 1.7)`
(GTK's symbols file stops at 4.12) and the GStreamer base/good plugins, and
recommends gstreamer1.0-libav (AAC) and xdg-desktop-portal. The `.rpm` is
built with the default features on Fedora: it gets its library requirements
automatically and states `gtk4 >= 4.20`, `libadwaita >= 1.8`, the GStreamer
plugins, `libsoup3` and `glib-networking` explicitly (Fedora only
recommends the last two, and without them there is no HTTPS source). Keep
those GTK and libadwaita minimums in step with the feature each package is
built with.

Install with `sudo apt install ./swell-player_*.deb` or
`sudo dnf install ./swell-player-*.rpm`, so the dependencies are pulled in.

## Windows and macOS

The release workflow builds both; the scripts can also run by hand on those
systems.

- **Windows** (MSYS2 UCRT64): first
  `packaging/windows/make-ico.sh data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg packaging/windows/swell.ico`,
  so that `build.rs` embeds the icon in `swell-player.exe`; then
  `cargo build --release --locked`, `packaging/windows/bundle.sh` (a
  self-contained folder with the GTK, GStreamer and GLib DLLs, plugins and
  translations) and Inno Setup on `packaging/windows/swell.iss`.
- **macOS** (Homebrew): `packaging/macos/bundle.sh` makes `Swell.app` and the
  `.dmg`, signed ad hoc. The Intel disk image is best effort: when that job
  fails, the release is published without it.

Both bundles include `BUNDLED-PACKAGES.txt` (each MSYS2 or Homebrew package
with its version, licence and source) and those packages' licence files. Its
source offer commits the maintainer to provide the sources on request; keep
the wording in the bundle scripts in line with what you can honour.

## Licence notices

`packaging/release.sh licenses` writes `target/release/THIRD-PARTY-LICENSES.txt`,
the licences of every Rust crate in the binary (needs
`cargo install cargo-about --version 0.9.2 --locked --features cli`). The
tarball, `.deb`, `.rpm` and the Windows and macOS bundles ship it, and each
release has it as an asset. Accepted licences are listed in `about.toml`: a
dependency under any other licence fails CI until it is reviewed and added
there.
