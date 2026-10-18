#!/usr/bin/env bash
# Builds a self-contained Windows folder of Swell from an MSYS2 UCRT64 shell.
#
#   packaging/windows/bundle.sh [--binary PATH] [--out DIR] [--zip FILE]
#
# The folder mirrors the MSYS2 prefix, which GLib, GTK, gdk-pixbuf and
# GStreamer locate relative to their DLLs at run time:
#
#   bin/                       swell-player.exe, gdbus.exe and every DLL they need
#   lib/gstreamer-1.0/         the GStreamer plugins playback uses
#   libexec/gstreamer-1.0/     gst-plugin-scanner.exe
#   lib/gdk-pixbuf-2.0/2.10.0/ SVG loader + loaders.cache (relative paths)
#   lib/gio/modules/           TLS backends (glib-networking)
#   share/glib-2.0/schemas/    compiled GTK schemas
#   share/icons/               Adwaita (no cursors) + hicolor
#   share/locale/              GTK, libadwaita and GLib translations
#   etc/ssl/                   CA bundle for TLS from GStreamer (libsoup)
#
# Needs: ntldd, glib-compile-schemas, gdk-pixbuf-query-loaders, rsvg-convert,
# zip (the packages installed by .github/workflows/release.yml).
set -euo pipefail

: "${MINGW_PREFIX:?run this from an MSYS2 UCRT64 shell}"
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
prefix=$MINGW_PREFIX
binary=$root/target/release/swell-player.exe
out=$root/dist/windows/Swell
zip=

while [[ $# -gt 0 ]]; do
    case $1 in
        --binary) binary=$2; shift ;;
        --out) out=$2; shift ;;
        --zip) zip=$2; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

[[ -f $binary ]] || { echo "$binary is missing: cargo build --release first" >&2; exit 1; }

log() { printf '==> %s\n' "$*"; }
warn() { printf '::warning::%s\n' "$*"; }
die() { printf '::error::%s\n' "$*" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"/{bin,lib/gstreamer-1.0,libexec/gstreamer-1.0,lib/gio/modules,etc/ssl/certs}
mkdir -p "$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders "$out"/share/{glib-2.0/schemas,icons,locale}

# --- Executables and modules (the roots of the DLL closure) ----------------------
log "executables"
cp "$binary" "$out/bin/swell-player.exe"
# gdbus.exe starts the per-session bus GApplication uses for single-instance
# and for handing tidal:// links to the running window.
cp "$prefix/bin/gdbus.exe" "$out/bin/"
cp "$prefix/libexec/gstreamer-1.0/gst-plugin-scanner.exe" "$out/libexec/gstreamer-1.0/"

log "GStreamer plugins"
# Required: playbin3 and what it plugs for TIDAL's streams (FLAC over plain
# HTTPS, Hi-Res as MPEG-DASH of fragmented MP4).
required_plugins=(
    coreelements typefindfunctions playback      # queue2/typefind, playbin3, decodebin3, urisourcebin
    audioconvert audioresample volume autodetect # conversions, normalisation, autoaudiosink
    soup adaptivedemux2 isomp4 flac audioparsers # souphttpsrc, dashdemux2, qtdemux, flacdec/flacparse
)
# Optional: sinks (at least one is checked below), AAC decoders for the
# "High"/"Low" qualities, ID3 tags some FLAC files carry.
optional_plugins=(wasapi2 wasapi directsound mediafoundation fdkaac faad id3demux pbtypes)
for name in "${required_plugins[@]}"; do
    src=$prefix/lib/gstreamer-1.0/libgst$name.dll
    [[ -f $src ]] || die "required GStreamer plugin $name is missing ($src)"
    cp "$src" "$out/lib/gstreamer-1.0/"
done
for name in "${optional_plugins[@]}"; do
    src=$prefix/lib/gstreamer-1.0/libgst$name.dll
    if [[ -f $src ]]; then
        cp "$src" "$out/lib/gstreamer-1.0/"
    else
        warn "optional GStreamer plugin $name not found"
    fi
done
if ! compgen -G "$out/lib/gstreamer-1.0/libgstwasapi*.dll" >/dev/null &&
    [[ ! -f $out/lib/gstreamer-1.0/libgstdirectsound.dll ]]; then
    die "no Windows audio sink plugin (wasapi2, wasapi or directsound)"
fi

log "gdk-pixbuf loaders"
# MSYS2 builds PNG, JPEG... into gdk-pixbuf itself; SVG (librsvg) is a module.
loaders_src=$prefix/lib/gdk-pixbuf-2.0/2.10.0/loaders
shopt -s nullglob
svg_loaders=("$loaders_src"/*svg*.dll)
shopt -u nullglob
[[ ${#svg_loaders[@]} -gt 0 ]] || die "librsvg's gdk-pixbuf loader is missing ($loaders_src)"
cp "${svg_loaders[@]}" "$out/lib/gdk-pixbuf-2.0/2.10.0/loaders/"

log "GIO TLS modules"
# libsoup (souphttpsrc, dashdemux2) gets TLS from glib-networking. The proxy
# modules are left out: they need GNOME's schemas or libproxy.
for module in libgiognutls.dll libgioopenssl.dll; do
    if [[ -f $prefix/lib/gio/modules/$module ]]; then
        cp "$prefix/lib/gio/modules/$module" "$out/lib/gio/modules/"
    else
        warn "GIO module $module not found"
    fi
done
compgen -G "$out/lib/gio/modules/*.dll" >/dev/null || die "no GIO TLS module (glib-networking)"

# --- DLL closure ----------------------------------------------------------------------
# GStreamer's soup and adaptivedemux2 plugins load libsoup with LoadLibrary at
# run time, so no import table names it: add it as a root.
log "DLL dependencies"
soup=$(compgen -G "$prefix/bin/libsoup-3.0-*.dll" | head -n 1 || true)
[[ -n $soup ]] || die "libsoup 3 is missing (mingw-w64-ucrt-x86_64-libsoup3)"
cp "$soup" "$out/bin/"
roots=("$out/bin/swell-player.exe" "$out/bin/gdbus.exe" "$out/bin/$(basename "$soup")"
    "$out"/libexec/gstreamer-1.0/*.exe "$out"/lib/gstreamer-1.0/*.dll
    "$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll "$out"/lib/gio/modules/*.dll)

prefix_win=$(cygpath -m "$prefix" | tr '[:upper:]' '[:lower:]')
# `ntldd -R` prints "name => C:\msys64\ucrt64\bin\name.dll (0x...)"; keep the
# DLLs under the MSYS2 prefix (system DLLs come with Windows).
{ ntldd -R "${roots[@]}" 2>/dev/null || true; } | tr -d '\r' |
    sed -n 's/^[[:space:]]*[^[:space:]]* => \(.*\) (0x[0-9a-fA-F]*)[[:space:]]*$/\1/p' |
    while IFS= read -r path; do
        mixed=$(cygpath -m "$path")
        lower=$(printf '%s' "$mixed" | tr '[:upper:]' '[:lower:]')
        if [[ $lower == "$prefix_win"/* ]]; then
            printf '%s\n' "$mixed"
        fi
    done | sort -uf >"$out/.dlls"
while IFS= read -r dll; do
    dll=$(cygpath -u "$dll")
    [[ -e $out/bin/$(basename "$dll") ]] || cp "$dll" "$out/bin/"
done <"$out/.dlls"
log "$(wc -l <"$out/.dlls") DLLs from $prefix"
rm "$out/.dlls"
# Completeness is proven by the smoke tests at the end, which run the bundled
# programs with nothing from MSYS2 on PATH.

# --- Data -----------------------------------------------------------------------------
log "loaders.cache"
# gdk-pixbuf in MSYS2 is built relocatable: module paths relative to the
# bundle root (the parent of bin/) are resolved at run time.
cache=$out/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache
out_win=$(cygpath -m "$out")
gdk-pixbuf-query-loaders "$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll | tr -d '\r' |
    sed -e '/^"[^"]*\.dll"$/ s|\\\\|/|g' |
    awk -v base="$out_win/" '
        BEGIN { lb = tolower(base) }
        /^"[^"]*\.dll"$/ {
            path = substr($0, 2, length($0) - 2)
            if (index(tolower(path), lb) == 1) path = substr(path, length(base) + 1)
            print "\"" path "\""
            next
        }
        { print }' >"$cache"
if grep -Eq '^"([A-Za-z]:)?/' "$cache"; then
    grep -E '^"([A-Za-z]:)?/' "$cache" >&2
    die "loaders.cache still has absolute paths"
fi
grep -q 'svg' "$cache" || die "loaders.cache has no SVG loader"
cat "$cache"

log "GIO module cache"
gio-querymodules "$out/lib/gio/modules" || warn "gio-querymodules failed (modules still load without a cache)"

log "GSettings schemas"
cp "$prefix"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$out/share/glib-2.0/schemas/"
glib-compile-schemas "$out/share/glib-2.0/schemas"

log "icon themes"
cp -r "$prefix/share/icons/Adwaita" "$out/share/icons/"
rm -rf "$out/share/icons/Adwaita/cursors"
mkdir -p "$out/share/icons/hicolor"
cp "$prefix/share/icons/hicolor/index.theme" "$out/share/icons/hicolor/"
for theme in Adwaita hicolor; do
    gtk4-update-icon-cache -q -t -f "$out/share/icons/$theme" ||
        warn "could not build the $theme icon cache"
done

log "translations"
for dir in "$prefix"/share/locale/*/LC_MESSAGES; do
    lang=$(basename "$(dirname "$dir")")
    for domain in gtk40 libadwaita glib20; do
        if [[ -f $dir/$domain.mo ]]; then
            mkdir -p "$out/share/locale/$lang/LC_MESSAGES"
            cp "$dir/$domain.mo" "$out/share/locale/$lang/LC_MESSAGES/"
        fi
    done
done

log "CA certificates"
ca=
for candidate in "$prefix/etc/ssl/certs/ca-bundle.crt" "$prefix/etc/ssl/cert.pem"; do
    if [[ -f $candidate ]]; then
        ca=$candidate
        break
    fi
done
[[ -n $ca ]] || die "no CA bundle (mingw-w64-ucrt-x86_64-ca-certificates)"
cp "$ca" "$out/etc/ssl/certs/ca-bundle.crt"
cp "$ca" "$out/etc/ssl/cert.pem"

if [[ -d $prefix/etc/fonts ]]; then
    log "fontconfig configuration"
    cp -r "$prefix/etc/fonts" "$out/etc/"
fi

log "icon, licence and readme"
"$root/packaging/windows/make-ico.sh" \
    "$root/data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" "$out/swell.ico"
# Text files people open on Windows get CRLF line ends.
sed 's/$/\r/' "$root/COPYING" >"$out/COPYING.txt"
sed 's/$/\r/' >"$out/README.txt" <<'EOF'
Swell - a native TIDAL client (GTK 4 + libadwaita + GStreamer)

Run bin\swell-player.exe. This folder is self-contained and can live
anywhere; settings, cache and the saved queue are kept in your user
profile, not here.

This build is not signed: Windows SmartScreen may warn the first time
("More info", then "Run anyway").

Swell is not affiliated with TIDAL. You need a paid TIDAL subscription.

Source code: https://github.com/oxakromax/swell-player
Licence: GPL-3.0-or-later (COPYING.txt).
EOF

# --- Smoke tests ----------------------------------------------------------------------
# Run the bundled programs with nothing from MSYS2 on PATH, so a missing DLL
# or plugin fails here and not on someone's machine. Windows looks for a
# program's DLLs in the program's own directory (bin/) first.
log "smoke tests"
cp "$prefix/bin/gst-inspect-1.0.exe" "$out/bin/"
inspect=$out/bin/gst-inspect-1.0.exe
registry_dir=$(mktemp -d)
registry_win=$(cygpath -w "$registry_dir")
bare_path="$out/bin:/c/Windows/System32:/c/Windows:/c/Windows/System32/Wbem"
elements=(playbin3 uridecodebin3 urisourcebin decodebin3 parsebin souphttpsrc dashdemux2 qtdemux
    flacparse flacdec audioconvert audioresample volume autoaudiosink typefind queue2)
unset_gst=(-u GST_PLUGIN_PATH -u GST_PLUGIN_PATH_1_0 -u GST_PLUGIN_SYSTEM_PATH -u GST_PLUGIN_SYSTEM_PATH_1_0
    -u GST_PLUGIN_SCANNER -u GST_PLUGIN_SCANNER_1_0)

# 1. Does GStreamer find the plugins by itself (relative to its DLL)?
#    Informative only: the app sets the variables of step 2 when bundled.
if env "${unset_gst[@]}" PATH="$bare_path" GST_REGISTRY_1_0="$registry_win\\relocated.bin" \
    "$inspect" --exists playbin3; then
    log "GStreamer finds the bundled plugins by itself"
else
    warn "GStreamer did not find the bundled plugins without GST_PLUGIN_SYSTEM_PATH_1_0"
fi

# 2. With the variables the app sets for a bundled run: required.
gst_env=(PATH="$bare_path"
    GST_PLUGIN_SYSTEM_PATH_1_0="$(cygpath -w "$out/lib/gstreamer-1.0")"
    GST_PLUGIN_SCANNER_1_0="$(cygpath -w "$out/libexec/gstreamer-1.0/gst-plugin-scanner.exe")"
    GST_REGISTRY_1_0="$registry_win\\bundle.bin")
for element in "${elements[@]}"; do
    env "${unset_gst[@]}" "${gst_env[@]}" "$inspect" --exists "$element" ||
        die "GStreamer element $element is not available in the bundle"
done
env "${unset_gst[@]}" "${gst_env[@]}" "$inspect" playbin3 >/dev/null
log "sinks and AAC decoders in the bundle:"
{ env "${unset_gst[@]}" "${gst_env[@]}" "$inspect" 2>/dev/null || true; } | tr -d '\r' |
    grep -E 'wasapi|directsound|aacdec|faad' || warn "no Windows sink or AAC decoder listed"
rm -f "$inspect"

# 3. The app starts: a missing DLL ends it with 0xC0000135 before main().
env "${unset_gst[@]}" "${gst_env[@]}" "$out/bin/swell-player.exe" --help >/dev/null ||
    die "swell-player.exe --help failed: the bundle is incomplete"
rm -rf "$registry_dir"

du -sh "$out"

if [[ -n $zip ]]; then
    log "zip $zip"
    mkdir -p "$(dirname "$zip")"
    zip_abs=$(cd "$(dirname "$zip")" && pwd)/$(basename "$zip")
    rm -f "$zip_abs"
    (cd "$(dirname "$out")" && zip -qr9 "$zip_abs" "$(basename "$out")")
    ls -l "$zip_abs"
fi
