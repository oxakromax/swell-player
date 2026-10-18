#!/bin/bash
# Builds Swell.app (and optionally a .dmg) from Homebrew's GTK stack.
#
#   packaging/macos/bundle.sh [--binary PATH] [--licenses FILE] [--out DIR] [--dmg FILE]
#
# Layout of Swell.app/Contents:
#
#   MacOS/Swell                  launcher (sh): environment, then exec swell-player
#   MacOS/swell-player           the binary
#   MacOS/gst-plugin-scanner     GStreamer's plugin scanner
#   Frameworks/*.dylib           every non-system library, relinked to @loader_path
#   PlugIns/gstreamer-1.0/       the GStreamer plugins playback uses
#   PlugIns/gdk-pixbuf/loaders/  SVG (and PNG/JPEG if not built in) loaders
#   PlugIns/gio-modules/         glib-networking's TLS backend
#   Resources/                   Info.plist data, icon, schemas, icon themes,
#                                CA certificates, loaders.cache template,
#                                translations of GTK, libadwaita, GLib, GStreamer
#   Resources/COPYING            Swell's licence (GPL-3.0-or-later)
#   Resources/THIRD-PARTY-LICENSES.txt  the Rust crates in swell-player (--licenses,
#                                made by `packaging/release.sh licenses`)
#   Resources/BUNDLED-PACKAGES.txt      every bundled Homebrew formula: version,
#                                licence, upstream, source; their licence files
#                                are in Resources/licenses/<formula>/
#
# The app is signed ad hoc (codesign -s -), not with a Developer ID, and not
# notarised: Gatekeeper refuses it on first open until it is allowed in
# System Settings > Privacy & Security ("Open Anyway"), or the quarantine
# flag is removed (xattr -dr com.apple.quarantine /Applications/Swell.app).
#
# Runs on the macOS bash 3.2: no associative arrays, no mapfile. Needs
# Homebrew, rsvg-convert, iconutil, jq (on GitHub's runners).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
brew=$(brew --prefix)
binary=$root/target/release/swell-player
licenses=$root/target/release/THIRD-PARTY-LICENSES.txt
out=$root/dist/macos
dmg=
app_id=io.github.oxakromax.SwellPlayer
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1)

while [ $# -gt 0 ]; do
    case $1 in
        --binary) binary=$2; shift ;;
        --licenses) licenses=$2; shift ;;
        --out) out=$2; shift ;;
        --dmg) dmg=$2; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

[ -f "$binary" ] || { echo "$binary is missing: cargo build --release first" >&2; exit 1; }
[ -s "$licenses" ] || {
    echo "$licenses is missing: run 'packaging/release.sh licenses' (needs cargo-about)" >&2
    exit 1
}
command -v jq >/dev/null || { echo "jq is needed (brew install jq)" >&2; exit 1; }

log() { printf '==> %s\n' "$*"; }
warn() { printf '::warning::%s\n' "$*"; }
die() { printf '::error::%s\n' "$*" >&2; exit 1; }

app=$out/Swell.app
contents=$app/Contents
macos=$contents/MacOS
fw=$contents/Frameworks
plugins=$contents/PlugIns
res=$contents/Resources

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
: >"$work/files"
# Records files copied from Homebrew, to find the formulae they come from.
from_brew() {
    local file
    for file; do
        realpath "$file" >>"$work/files" 2>/dev/null || true
    done
}

rm -rf "$app"
mkdir -p "$macos" "$fw" "$plugins/gstreamer-1.0" "$plugins/gdk-pixbuf/loaders" "$plugins/gio-modules" \
    "$res/share/glib-2.0/schemas" "$res/share/icons" "$res/etc/ssl/certs"

# --- Relinking -------------------------------------------------------------------------
is_system() {
    case $1 in
        /usr/lib/* | /System/*) return 0 ;;
    esac
    return 1
}

# Install names FILE links against, without its own id.
macho_deps() {
    local id
    id=$(otool -D "$1" | sed -n 2p)
    otool -L "$1" | tail -n +2 | sed -E 's/^[[:space:]]+//; s/ \(compatibility version.*$//' |
        while IFS= read -r dep; do
            if [ "$dep" != "$id" ]; then printf '%s\n' "$dep"; fi
        done
}

macho_files() {
    find "$contents" -type f | while IFS= read -r file; do
        case $(file -b "$file") in
            *Mach-O*) printf '%s\n' "$file" ;;
        esac
    done
}

rpaths() {
    otool -l "$1" | awk '/cmd LC_RPATH/ { getline; getline; print $2 }'
}

# The file an install name DEP of FROM refers to.
resolve() {
    local dep=$1 from=$2 dir name rp cand=
    dir=$(dirname "$from")
    case $dep in
        @loader_path/* | @executable_path/*)
            cand=$dir/${dep#*/}
            [ -e "$cand" ] || cand=$(dirname "$(realpath "$from")")/${dep#*/}
            ;;
        @rpath/*)
            name=${dep#@rpath/}
            for rp in $(rpaths "$from"); do
                rp=${rp/@loader_path/$dir}
                rp=${rp/@executable_path/$dir}
                if [ -e "$rp/$name" ]; then
                    cand=$rp/$name
                    break
                fi
            done
            [ -n "$cand" ] || cand=$brew/lib/$name
            ;;
        *) cand=$dep ;;
    esac
    [ -e "$cand" ] || die "cannot resolve $dep (needed by $from)"
    printf '%s\n' "$cand"
}

# install_name_tool, quiet about the signature it invalidates (all files
# are signed again at the end).
int() {
    local err
    if ! err=$(install_name_tool "$@" 2>&1); then
        printf '%s\n' "$err" >&2
        return 1
    fi
    printf '%s\n' "$err" | grep -v -e 'invalidate the code signature' -e '^$' >&2 || true
}

# relink FILE ORIG TOFW: copies the non-system libraries ORIG (FILE's source)
# needs into Frameworks/, recursively, and points FILE at them through
# @loader_path/TOFW (TOFW: path from FILE's directory to Frameworks/).
relink() {
    local file=$1 orig=$2 tofw=$3 dep src name rp
    chmod u+w "$file"
    for dep in $(macho_deps "$orig"); do
        is_system "$dep" && continue
        src=$(resolve "$dep" "$orig")
        name=$(basename "$dep")
        if [ ! -e "$fw/$name" ]; then
            cp -L "$src" "$fw/$name"
            from_brew "$src"
            chmod u+w "$fw/$name"
            int -id "@rpath/$name" "$fw/$name"
            relink "$fw/$name" "$src" ""
        fi
        int -change "$dep" "@loader_path${tofw:+/$tofw}/$name" "$file"
    done
    # Only Frameworks/ on the run path: dyld also tries the caller's
    # LC_RPATHs for a bare-name dlopen() (GStreamer's soup plugins open
    # libsoup that way), and Homebrew's would pick up its own copies.
    for rp in $(rpaths "$file"); do
        int -delete_rpath "$rp" "$file"
    done
    int -add_rpath "@loader_path${tofw:+/$tofw}" "$file"
}

# add SRC DEST TOFW: copies a Mach-O file into the bundle and relinks it.
add() {
    local src=$1 dest=$2 tofw=$3
    cp -L "$src" "$dest"
    from_brew "$src"
    relink "$dest" "$src" "$tofw"
}

# --- Executables, plugins, modules -------------------------------------------------------
log "binary"
add "$binary" "$macos/swell-player" ../Frameworks

scanner=
for candidate in "$(brew --prefix gstreamer)/libexec/gstreamer-1.0/gst-plugin-scanner" \
    "$brew/libexec/gstreamer-1.0/gst-plugin-scanner"; do
    if [ -x "$candidate" ]; then
        scanner=$candidate
        break
    fi
done
[ -n "$scanner" ] || die "gst-plugin-scanner not found"
add "$scanner" "$macos/gst-plugin-scanner" ../Frameworks

log "GStreamer plugins"
plugin_file() {
    local ext
    for ext in dylib so; do
        if [ -f "$brew/lib/gstreamer-1.0/libgst$1.$ext" ]; then
            printf '%s\n' "$brew/lib/gstreamer-1.0/libgst$1.$ext"
            return 0
        fi
    done
    return 1
}
# Required: playbin3 and what it plugs for TIDAL's streams (FLAC over plain
# HTTPS, Hi-Res as MPEG-DASH of fragmented MP4), and the Core Audio sink.
for name in coreelements typefindfunctions playback audioconvert audioresample volume autodetect \
    soup adaptivedemux2 isomp4 flac audioparsers osxaudio; do
    src=$(plugin_file "$name") || die "required GStreamer plugin $name is missing"
    add "$src" "$plugins/gstreamer-1.0/$(basename "$src")" ../../Frameworks
done
# Optional: AAC decoders for the "High"/"Low" qualities (Audio Toolbox via
# applemedia, or faad), ID3 tags some FLAC files carry.
for name in applemedia faad id3demux pbtypes; do
    if src=$(plugin_file "$name"); then
        add "$src" "$plugins/gstreamer-1.0/$(basename "$src")" ../../Frameworks
    else
        warn "optional GStreamer plugin $name not found"
    fi
done

# GStreamer's soup and adaptivedemux2 plugins dlopen() libsoup by its bare
# name; the launcher puts Frameworks/ on DYLD_FALLBACK_LIBRARY_PATH for it.
log "libsoup (loaded at run time)"
soup=
for file in "$(brew --prefix libsoup)"/lib/libsoup-3.0.*.dylib; do
    if [ -f "$file" ]; then
        soup=$file
        break
    fi
done
[ -n "$soup" ] || die "libsoup 3 not found (brew install libsoup)"
soup_name=$(basename "$soup")
# Bare-name lookups use the name the plugins ask for: libsoup-3.0.0.dylib.
[ -e "$fw/libsoup-3.0.0.dylib" ] || add "$soup" "$fw/libsoup-3.0.0.dylib" ""
[ "$soup_name" = libsoup-3.0.0.dylib ] || [ -e "$fw/$soup_name" ] || cp "$fw/libsoup-3.0.0.dylib" "$fw/$soup_name"

log "gdk-pixbuf loaders"
loaders_dir=$brew/lib/gdk-pixbuf-2.0/2.10.0/loaders
loaders=
for file in "$loaders_dir"/*; do
    case $(basename "$file") in
        *svg* | *png* | *jpeg*)
            add "$file" "$plugins/gdk-pixbuf/loaders/$(basename "$file")" ../../../Frameworks
            loaders="$loaders $file"
            ;;
    esac
done
case $loaders in *svg*) ;; *) die "librsvg's gdk-pixbuf loader is missing ($loaders_dir)" ;; esac
# Homebrew's gdk-pixbuf is not relocatable: loaders.cache needs absolute
# paths, so the launcher fills in @PLUGINS@ at each start. The cache is
# made from Homebrew's own loaders (same files, same contents).
# shellcheck disable=SC2086 # $loaders is a list of paths without spaces
gdk-pixbuf-query-loaders $loaders |
    awk '/^"[^"]*\.(so|dylib)"$/ {
            path = substr($0, 2, length($0) - 2)
            n = split(path, parts, "/")
            print "\"@PLUGINS@/gdk-pixbuf/loaders/" parts[n] "\""
            next
        }
        /^# LoaderDir/ { next }
        { print }' >"$res/loaders.cache.in"
grep -q '@PLUGINS@/gdk-pixbuf/loaders/.*svg' "$res/loaders.cache.in" || die "loaders.cache has no SVG loader"

log "GIO TLS module"
found=false
for file in "$brew"/lib/gio/modules/libgiognutls.* "$brew"/lib/gio/modules/libgioopenssl.*; do
    [ -f "$file" ] || continue
    add "$file" "$plugins/gio-modules/$(basename "$file")" ../../Frameworks
    found=true
done
$found || die "no GIO TLS module (brew install glib-networking)"

# --- Data ----------------------------------------------------------------------------------
log "GSettings schemas"
cp "$brew"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$res/share/glib-2.0/schemas/"
from_brew "$brew"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml
glib-compile-schemas "$res/share/glib-2.0/schemas"

log "icon themes"
# -H: Homebrew links the theme directory itself as a symlink.
cp -RH "$brew/share/icons/Adwaita" "$res/share/icons/"
rm -rf "$res/share/icons/Adwaita/cursors"
mkdir -p "$res/share/icons/hicolor"
cp "$brew/share/icons/hicolor/index.theme" "$res/share/icons/hicolor/"
from_brew "$brew/share/icons/Adwaita/index.theme" "$brew/share/icons/hicolor/index.theme"
for theme in Adwaita hicolor; do
    gtk4-update-icon-cache -q -t -f "$res/share/icons/$theme" || warn "could not build the $theme icon cache"
done

log "CA certificates"
ca=
for candidate in "$(brew --prefix ca-certificates)/share/ca-certificates/cacert.pem" \
    "$brew/etc/ca-certificates/cert.pem"; do
    if [ -f "$candidate" ]; then
        ca=$candidate
        break
    fi
done
[ -n "$ca" ] || die "no CA bundle (brew install ca-certificates)"
cp -L "$ca" "$res/etc/ssl/cert.pem"
cp -L "$ca" "$res/etc/ssl/certs/ca-bundle.crt"
from_brew "$ca"

if [ -d "$brew/etc/fonts" ]; then
    cp -RL "$brew/etc/fonts" "$res/etc/"
    # $brew/etc is outside the Cellar: name the formula.
    from_brew "$(brew --prefix fontconfig)/lib/libfontconfig.1.dylib"
fi

log "translations"
# The catalogues of the libraries (Swell's own are in the binary).
find -L "$brew/share/locale" -path '*/LC_MESSAGES/*.mo' 2>/dev/null |
    while IFS= read -r mo; do
        case $(basename "$mo") in
            gtk40.mo | libadwaita.mo | glib20.mo | gstreamer-1.0.mo | gst-plugins-base-1.0.mo | \
                gst-plugins-good-1.0.mo | gst-plugins-bad-1.0.mo) ;;
            *) continue ;;
        esac
        lang_dir=${mo#"$brew/share/locale/"}
        mkdir -p "$res/share/locale/$(dirname "$lang_dir")"
        cp -L "$mo" "$res/share/locale/$lang_dir"
        from_brew "$mo"
    done
[ -f "$res/share/locale/es/LC_MESSAGES/gtk40.mo" ] || warn "GTK's Spanish translation was not found"

log "icon"
iconset=$(mktemp -d)/swell.iconset
mkdir -p "$iconset"
svg=$root/data/icons/scalable/apps/$app_id.svg
for size in 16 32 128 256 512; do
    rsvg-convert -w $size -h $size -o "$iconset/icon_${size}x${size}.png" "$svg"
    rsvg-convert -w $((size * 2)) -h $((size * 2)) -o "$iconset/icon_${size}x${size}@2x.png" "$svg"
done
iconutil -c icns -o "$res/swell.icns" "$iconset"
rm -rf "$(dirname "$iconset")"

log "licences"
cp "$root/COPYING" "$res/COPYING"
cp "$licenses" "$res/THIRD-PARTY-LICENSES.txt"
# The formulae of the files copied above (Cellar/<formula>/<version>/...).
sed -n "s|^$(realpath "$brew")/Cellar/\([^/]*\)/\([^/]*\)/.*|\1 \2|p" "$work/files" | sort -u >"$work/formulae"
[ -s "$work/formulae" ] || die "found no Homebrew formula for the bundled files"
# shellcheck disable=SC2046 # formula names have no spaces
brew info --json=v2 $(cut -d' ' -f1 "$work/formulae") >"$work/info.json"
{
    cat <<'EOF'
Swell for macOS carries these libraries and programs from Homebrew
(https://brew.sh/), relinked to load from the app but otherwise unchanged.
Each is listed with its version, its licence and its upstream project;
their licence files are in Contents/Resources/licenses/.

Source code: Homebrew builds each formula from the upstream source given
below, with the build steps and patches of the formula file listed with it.
If any of these is no longer available, ask for it at
https://github.com/oxakromax/swell-player/issues and the Swell maintainers
will provide it, for at least three years after this release.

EOF
    while read -r formula version; do
        jq -r --arg f "$formula" --arg v "$version" '.formulae[] | select(.name == $f) |
            "\($f) \($v)\n" +
            "    Licence:  \(.license // "see the licence files")\n" +
            "    Upstream: \(.homepage)\n" +
            "    Source:   \(.urls.stable.url // "")\n" +
            "    Formula:  https://github.com/Homebrew/homebrew-core/blob/HEAD/\(.ruby_source_path // ("Formula/" + $f + ".rb"))\n"' \
            "$work/info.json"
    done <"$work/formulae"
} >"$res/BUNDLED-PACKAGES.txt"
# Homebrew keeps each formula's licence files at the top of its keg.
while read -r formula version; do
    keg=$(realpath "$brew")/Cellar/$formula/$version
    mkdir -p "$res/licenses/$formula"
    found=false
    for file in "$keg"/*; do
        [ -f "$file" ] || continue
        case $(basename "$file" | tr '[:lower:]' '[:upper:]') in
            COPYING* | LICENSE* | LICENCE* | NOTICE* | COPYRIGHT* | LGPL* | GPL* | MPL* | MIT*)
                cp -L "$file" "$res/licenses/$formula/"
                found=true
                ;;
        esac
    done
    $found || warn "$formula keeps no licence file (BUNDLED-PACKAGES.txt names its licence)"
done <"$work/formulae"
# The CA list (Mozilla's, MPL-2.0) and the Adwaita icons (CC-BY-SA-3.0 or
# LGPL-3.0-or-later) are data whose formulae may not keep the text.
mkdir -p "$res/licenses/ca-certificates" "$res/licenses/adwaita-icon-theme"
cp "$root/packaging/licenses/MPL-2.0.txt" "$res/licenses/ca-certificates/"
cp "$root/packaging/licenses/CC-BY-SA-3.0.txt" "$res/licenses/adwaita-icon-theme/"
log "$(wc -l <"$work/formulae" | tr -d ' ') Homebrew formulae bundled"

# --- Launcher and Info.plist ---------------------------------------------------------------
log "launcher"
cat >"$macos/Swell" <<'EOF'
#!/bin/sh
# Swell.app launcher: points GLib, GTK and GStreamer at the bundled copies,
# then runs the real binary.
contents=$(cd "$(dirname "$0")/.." && pwd -P)
res=$contents/Resources
plugins=$contents/PlugIns
cache=$HOME/Library/Caches/io.github.oxakromax.SwellPlayer
mkdir -p "$cache"

# gdk-pixbuf (built non-relocatable) wants absolute module paths.
escaped=$(printf '%s\n' "$plugins" | sed 's/[\\&|]/\\&/g')
sed "s|@PLUGINS@|$escaped|g" "$res/loaders.cache.in" >"$cache/loaders.cache.$$" &&
    mv -f "$cache/loaders.cache.$$" "$cache/loaders.cache"

# GStreamer's soup plugins dlopen() libsoup by its bare name.
export DYLD_FALLBACK_LIBRARY_PATH="$contents/Frameworks:/usr/lib"
export GDK_PIXBUF_MODULE_FILE="$cache/loaders.cache"
export GDK_PIXBUF_MODULEDIR="$plugins/gdk-pixbuf/loaders"
export GIO_MODULE_DIR="$plugins/gio-modules"
export GSETTINGS_SCHEMA_DIR="$res/share/glib-2.0/schemas"
export XDG_DATA_DIRS="$res/share"
export GST_PLUGIN_SYSTEM_PATH_1_0="$plugins/gstreamer-1.0"
export GST_PLUGIN_SCANNER_1_0="$contents/MacOS/gst-plugin-scanner"
export GST_REGISTRY_1_0="$cache/gstreamer-registry-$(uname -m).bin"
unset GST_PLUGIN_PATH GST_PLUGIN_PATH_1_0 GST_PLUGIN_SYSTEM_PATH GST_PLUGIN_SCANNER
if [ -d "$res/etc/fonts" ]; then
    export FONTCONFIG_PATH="$res/etc/fonts"
fi

# Older macOS passes a process serial number to apps opened from Finder.
for arg; do
    shift
    case $arg in
        -psn_*) ;;
        *) set -- "$@" "$arg" ;;
    esac
done
exec "$contents/MacOS/swell-player" "$@"
EOF
chmod 755 "$macos/Swell"

# Every bundled Mach-O declares the oldest macOS it runs on; the bundle can
# claim no less than the newest of them (Homebrew builds for the runner's).
min_macos=$(macho_files |
    while IFS= read -r file; do
        otool -l "$file" 2>/dev/null | awk '/LC_BUILD_VERSION/ { found = 1 } found && $1 == "minos" { print $2; exit }'
    done | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
[ -n "$min_macos" ] || min_macos=11.0
log "minimum macOS: $min_macos"

log "Info.plist"
cat >"$contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleLocalizations</key>
  <array>
    <string>en</string>
    <string>es</string>
  </array>
  <key>CFBundleDisplayName</key>
  <string>Swell</string>
  <key>CFBundleName</key>
  <string>Swell</string>
  <key>CFBundleExecutable</key>
  <string>Swell</string>
  <key>CFBundleIconFile</key>
  <string>swell</string>
  <key>CFBundleIdentifier</key>
  <string>$app_id</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSMinimumSystemVersion</key>
  <string>$min_macos</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Swell contributors. GPL-3.0-or-later.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>TIDAL link</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>tidal</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
EOF
plutil -lint "$contents/Info.plist"

# --- Checks ----------------------------------------------------------------------------------
log "checking links"
# A function, not inline: bash 3.2 can't parse a `case` inside $(...).
bad_links() {
    local file dep
    macho_files | while IFS= read -r file; do
        for dep in $(macho_deps "$file"); do
            case $dep in
                @loader_path/*)
                    [ -e "$(dirname "$file")/${dep#@loader_path/}" ] || echo "$file: $dep does not exist"
                    ;;
                /usr/lib/* | /System/*) ;;
                *) echo "$file: links $dep" ;;
            esac
        done
    done
}
bad=$(bad_links)
[ -z "$bad" ] || { printf '%s\n' "$bad" >&2; die "the bundle links libraries outside itself"; }
log "$(find "$fw" -type f | wc -l | tr -d ' ') libraries in Frameworks"

sign_all() {
    local file err
    macho_files | while IFS= read -r file; do
        if ! err=$(codesign --force --sign - "$file" 2>&1); then
            printf '%s\n' "$err" >&2
            die "codesign failed for $file"
        fi
    done
}

# Smoke tests: the bundled GStreamer and the app itself, with dyld printing
# every library it loads. Anything from Homebrew's prefix means a library
# was missed (Homebrew is installed here, so it would hide the gap).
log "smoke tests"
add "$(brew --prefix gstreamer)/bin/gst-inspect-1.0" "$macos/gst-inspect-1.0" ../Frameworks
sign_all
test_cache=$(mktemp -d)
run_bundled() {
    env -u GST_PLUGIN_PATH -u GST_PLUGIN_PATH_1_0 -u GST_PLUGIN_SYSTEM_PATH \
        DYLD_FALLBACK_LIBRARY_PATH="$fw:/usr/lib" \
        DYLD_PRINT_LIBRARIES=1 \
        GIO_MODULE_DIR="$plugins/gio-modules" \
        GST_PLUGIN_SYSTEM_PATH_1_0="$plugins/gstreamer-1.0" \
        GST_PLUGIN_SCANNER_1_0="$macos/gst-plugin-scanner" \
        GST_REGISTRY_1_0="$test_cache/registry.bin" \
        "$@"
}
for element in playbin3 uridecodebin3 urisourcebin decodebin3 parsebin souphttpsrc dashdemux2 qtdemux \
    flacparse flacdec audioconvert audioresample volume autoaudiosink osxaudiosink typefind queue2; do
    run_bundled "$macos/gst-inspect-1.0" --exists "$element" 2>>"$test_cache/dyld.log" ||
        die "GStreamer element $element is not available in the bundle"
done
# Instantiating souphttpsrc makes the plugin dlopen() libsoup.
run_bundled "$macos/gst-inspect-1.0" souphttpsrc >/dev/null 2>>"$test_cache/dyld.log"
run_bundled "$macos/gst-inspect-1.0" 2>/dev/null | grep -E 'osxaudio|atdec|faad' || warn "no AAC decoder listed"
# The binary directly (dyld variables do not survive /bin/sh, which SIP
# protects), then through the launcher.
run_bundled "$macos/swell-player" --help >/dev/null 2>>"$test_cache/dyld.log" || die "swell-player --help failed"
"$macos/Swell" --help >/dev/null || die "the launcher failed (Swell --help)"
if grep -E "$brew/|/usr/local/" "$test_cache/dyld.log" | grep -v "$app" >&2; then
    die "the bundle loaded libraries from Homebrew (above)"
fi
grep -q 'libsoup-3.0' "$test_cache/dyld.log" || warn "libsoup was not loaded during the smoke test"
rm -f "$macos/gst-inspect-1.0"
rm -rf "$test_cache" "$HOME/Library/Caches/$app_id/gstreamer-registry-$(uname -m).bin"

log "signing (ad hoc)"
sign_all
codesign --force --deep --sign - "$app"
codesign --verify --deep --verbose=2 "$app"
du -sh "$app"

if [ -n "$dmg" ]; then
    log "disk image $dmg"
    mkdir -p "$(dirname "$dmg")"
    stage=$(mktemp -d)
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    rm -f "$dmg"
    # hdiutil is known to fail now and then on CI machines ("Resource busy").
    for attempt in 1 2 3 4 5; do
        if hdiutil create -volname "Swell $version" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"; then
            break
        fi
        [ $attempt -lt 5 ] || die "hdiutil create failed"
        sleep $((attempt * 5))
    done
    rm -rf "$stage"
    ls -l "$dmg"
fi
