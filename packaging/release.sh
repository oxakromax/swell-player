#!/usr/bin/env bash
# Linux release artefacts for Swell, shared by .github/workflows/release.yml
# and local builds (so both run the same commands).
#
#   packaging/release.sh version            prints the version in Cargo.toml
#   packaging/release.sh check-tag TAG      fails unless TAG is v<version>
#   packaging/release.sh build [ARGS...]    cargo build --release --locked ARGS
#                                           (e.g. --no-default-features)
#   packaging/release.sh licenses [FILE]    third-party notices of the Rust crates, with
#                                           cargo-about (default target/release/THIRD-PARTY-LICENSES.txt)
#   packaging/release.sh tarball [OUTDIR]   swell-player-<v>-linux-<arch>.tar.gz
#   packaging/release.sh deb [OUTDIR]       .deb with cargo-deb (metadata in Cargo.toml)
#   packaging/release.sh rpm [OUTDIR]       .rpm with cargo-generate-rpm (metadata in Cargo.toml)
#   packaging/release.sh checksums DIR      writes DIR/SHA256SUMS
#
# OUTDIR defaults to dist/. The tarball, .deb and .rpm all package the
# binary and THIRD-PARTY-LICENSES.txt from target/release, so run `build`
# first (`licenses` runs by itself when the notices are missing and
# cargo-about is installed). The .deb and the tarball of a release come
# from a `--no-default-features` build (GTK 4.18 and libadwaita 1.7 are
# enough) on Debian 13, so they run on Debian 13, Ubuntu 25.04 and
# anything newer. The Windows and macOS bundles take the same notices
# (packaging/windows/bundle.sh, packaging/macos/bundle.sh).
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
bin_name=swell-player
licenses_file=$root/target/release/THIRD-PARTY-LICENSES.txt

die() {
    echo "release.sh: $*" >&2
    exit 1
}

version() {
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1
}

arch() {
    uname -m
}

binary() {
    local path=$root/target/release/$bin_name
    [[ -x $path ]] || die "$path is missing: run 'packaging/release.sh build' first"
    printf '%s\n' "$path"
}

has_metadata() {
    grep -q "^\[package\.metadata\.$1\]" "$root/Cargo.toml"
}

cmd_check_tag() {
    local tag=${1:?usage: release.sh check-tag TAG} want
    want=v$(version)
    [[ $tag == "$want" ]] || die "tag $tag does not match Cargo.toml (expected $want)"
    echo "tag $tag matches Cargo.toml"
}

cmd_build() {
    cargo build --release --locked --manifest-path "$root/Cargo.toml" "$@"
}

# The licences of the crates compiled into the binary (about.toml lists the
# accepted ones and every release target; packaging/about.hbs is the
# template). Fails on a crate whose licence is not accepted or not found.
cmd_licenses() {
    local file=${1:-$licenses_file}
    command -v cargo-about >/dev/null ||
        die "cargo-about is missing (cargo install cargo-about --locked --features cli)"
    mkdir -p "$(dirname "$file")"
    cargo about generate --locked --fail \
        --manifest-path "$root/Cargo.toml" --config "$root/about.toml" \
        --output-file "$file" "$root/packaging/about.hbs"
    grep -q '^Used by:' "$file" || die "$file lists no crates"
    echo "$file"
}

# The notices the packages ship, made now if they are missing.
licenses() {
    if [[ ! -s $licenses_file ]]; then
        command -v cargo-about >/dev/null ||
            die "$licenses_file is missing: run 'packaging/release.sh licenses' (needs cargo-about)"
        cmd_licenses >/dev/null
    fi
    printf '%s\n' "$licenses_file"
}

# A relocatable tree (bin/, share/) plus install.sh, which copies it into a
# prefix (/usr/local by default, ~/.local with --user).
cmd_tarball() {
    local out=${1:-$root/dist} name stage notices
    name=$bin_name-$(version)-linux-$(arch)
    notices=$(licenses)
    mkdir -p "$out"
    out=$(cd "$out" && pwd)
    stage=$(mktemp -d)

    local dir=$stage/$name
    # The files `make install` puts in place, under bin/ and share/.
    make -C "$root" --no-print-directory install DESTDIR="$dir" PREFIX= >/dev/null
    install -Dm644 "$root/README.md" "$dir/README.md"
    install -Dm644 "$root/COPYING" "$dir/COPYING"
    install -Dm644 "$notices" "$dir/THIRD-PARTY-LICENSES.txt"
    # Next to COPYING, where `make install` puts it (install.sh copies share/).
    install -Dm644 "$notices" "$dir/share/licenses/$bin_name/THIRD-PARTY-LICENSES.txt"
    write_install_script >"$dir/install.sh"
    chmod 755 "$dir/install.sh"

    tar -C "$stage" --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name"
    rm -rf "$stage"
    echo "$out/$name.tar.gz"
}

write_install_script() {
    cat <<'EOF'
#!/bin/sh
# Installs Swell from this directory.
#
#   ./install.sh                 into /usr/local (run with sudo)
#   ./install.sh --user          into ~/.local
#   ./install.sh --prefix DIR    into DIR
#   add --uninstall to remove what these commands installed
#
# Swell needs GTK >= 4.18, libadwaita >= 1.7, glibc >= 2.41 (Debian 13,
# Ubuntu 25.04 or newer) and GStreamer with the "base" and "good" plugin
# sets (gst-plugins-good has the DASH demuxer used for Hi-Res streams).
# Install them with your distribution's packages.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=/usr/local
uninstall=false

while [ $# -gt 0 ]; do
    case $1 in
        --user) prefix=$HOME/.local ;;
        --prefix) [ $# -ge 2 ] || { echo "--prefix needs a directory" >&2; exit 2; }; prefix=$2; shift ;;
        --prefix=*) prefix=${1#--prefix=} ;;
        --uninstall) uninstall=true ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

# Everything under bin/ and share/ in this directory, by relative path.
files=$(cd "$here" && find bin share -type f | sort)

if $uninstall; then
    echo "$files" | while read -r file; do
        rm -f "$prefix/$file"
    done
else
    echo "$files" | while read -r file; do
        mode=644
        [ "$file" = bin/swell-player ] && mode=755
        mkdir -p "$(dirname "$prefix/$file")"
        cp "$here/$file" "$prefix/$file.tmp"
        chmod "$mode" "$prefix/$file.tmp"
        mv -f "$prefix/$file.tmp" "$prefix/$file"
    done
fi

# Refresh the caches that desktops read; none of these are required.
command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
    [ -f "$prefix/share/icons/hicolor/index.theme" ] &&
    gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true

if $uninstall; then
    echo "Swell removed from $prefix"
else
    echo "Swell installed in $prefix (run: $prefix/bin/swell-player)"
fi
EOF
}

cmd_deb() {
    local out=${1:-$root/dist}
    has_metadata deb || die "Cargo.toml has no [package.metadata.deb] section"
    command -v cargo-deb >/dev/null || die "cargo-deb is missing (cargo install cargo-deb --locked)"
    binary >/dev/null
    licenses >/dev/null
    mkdir -p "$out"
    # --no-build: package the binary `build` made (release profile, stripped).
    cargo deb --manifest-path "$root/Cargo.toml" --no-build --no-strip --output "$out"
}

cmd_rpm() {
    local out=${1:-$root/dist}
    has_metadata generate-rpm || die "Cargo.toml has no [package.metadata.generate-rpm] section"
    command -v cargo-generate-rpm >/dev/null ||
        die "cargo-generate-rpm is missing (cargo install cargo-generate-rpm --locked)"
    binary >/dev/null
    licenses >/dev/null
    mkdir -p "$out"
    # Assets in the metadata name target/release/..., relative to the crate.
    (cd "$root" && cargo generate-rpm --output "$out")
}

cmd_checksums() {
    local dir=${1:?usage: release.sh checksums DIR}
    (
        cd "$dir"
        rm -f SHA256SUMS
        sums=$(find . -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort | xargs -r sha256sum)
        printf '%s\n' "$sums" >SHA256SUMS
        cat SHA256SUMS
    )
}

command=${1:-}
[[ $# -gt 0 ]] && shift
case $command in
    version) version ;;
    check-tag) cmd_check_tag "$@" ;;
    build) cmd_build "$@" ;;
    licenses) cmd_licenses "$@" ;;
    tarball) cmd_tarball "$@" ;;
    deb) cmd_deb "$@" ;;
    rpm) cmd_rpm "$@" ;;
    checksums) cmd_checksums "$@" ;;
    *) sed -n '2,23p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
esac
