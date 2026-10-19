#!/bin/sh
# Installs a Swell update on macOS. Swell (src/update/install.rs) writes
# this script next to its downloads and starts it as it quits:
#
#   install-macos.sh PID DMG APP VERSION RESTART
#
# Waits for Swell (process PID) to exit, copies Swell.app out of the disk
# image DMG next to APP under a temporary name, checks that the copy is
# VERSION, then swaps it in: the old app is moved aside only once the new
# one is complete and no Swell runs from it, and removed once the new one
# is in place. With RESTART=1, opens the app again, whether the update
# went in or not. The arguments are only used quoted.
set -u

pid=$1 dmg=$2 app=$3 version=$4 restart=$5
here=$(cd "$(dirname "$0")" && pwd -P)
exec >>"$here/install.log" 2>&1
echo "$(date '+%Y-%m-%d %H:%M:%S') installing Swell $version into $app"

# For a restart Swell is gone unless this opens it again, so it does on
# every way out: the new app, or the old one that stayed or was put back.
lock=$here/install.lock
locked=0 mnt=
cleanup() {
    if [ -n "$mnt" ]; then
        hdiutil detach "$mnt" -quiet || hdiutil detach "$mnt" -force -quiet
        rmdir "$mnt" 2>/dev/null
    fi
    if [ "$locked" = 1 ]; then
        rmdir "$lock"
    fi
    if [ "$restart" = 1 ] && [ -d "$app" ]; then
        open "$app"
    fi
}
trap cleanup EXIT

case $version in
    '' | *[!0-9.]*)
        echo "not a version: $version"
        exit 1
        ;;
esac

# One installation at a time: two Swells may quit with the same update. A
# lock older than ten minutes was left by one that was cut short.
if [ -n "$(find "$lock" -maxdepth 0 -mmin +10 2>/dev/null)" ]; then
    rmdir "$lock"
fi
if ! mkdir "$lock" 2>/dev/null; then
    echo "another installation is running"
    exit 1
fi
locked=1

# Swell exits within seconds of starting this; give up after a minute.
tries=0
while kill -0 "$pid" 2>/dev/null; do
    tries=$((tries + 1))
    if [ "$tries" -gt 600 ]; then
        echo "Swell did not exit"
        # It still runs: nothing to open again.
        restart=0
        exit 1
    fi
    sleep 0.1
done

bundle_version() {
    /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$1/Contents/Info.plist" 2>/dev/null
}
if [ "$(bundle_version "$app")" = "$version" ]; then
    echo "Swell $version is installed already"
    exit 0
fi

mnt=$(mktemp -d "${TMPDIR:-/tmp}/swell-update.XXXXXX") || exit 1
if ! hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$dmg" >/dev/null; then
    echo "could not open the disk image"
    exit 1
fi

parent=$(dirname "$app")
name=$(basename "$app")
new=$parent/.$name.new
old=$parent/.$name.old
# What an earlier attempt left must go: ditto would merge into it, and mv
# would move the app into it.
rm -rf "$new" "$old"
if [ -e "$new" ] || [ -e "$old" ]; then
    echo "cannot remove an earlier attempt ($new, $old)"
    exit 1
fi
if ! ditto "$mnt/Swell.app" "$new"; then
    echo "could not copy the new app"
    rm -rf "$new"
    exit 1
fi
if [ "$(bundle_version "$new")" != "$version" ] || [ ! -x "$new/Contents/MacOS/swell-player" ]; then
    echo "the disk image does not hold Swell $version"
    rm -rf "$new"
    exit 1
fi
xattr -dr com.apple.quarantine "$new" 2>/dev/null

# A Swell opened again meanwhile, or a second instance, still loads files
# from the old app: leave it be. The next run stages the update again.
# (ps prints each program's full path.)
if ps -axww -o comm= | grep -Fq "$app/Contents/MacOS/"; then
    echo "Swell is running again; installing later"
    rm -rf "$new"
    exit 1
fi
if ! mv "$app" "$old"; then
    echo "could not move the old app aside"
    rm -rf "$new"
    exit 1
fi
if ! mv "$new" "$app"; then
    echo "could not put the new app in place; keeping the old one"
    mv "$old" "$app" || echo "could not put the old app back; it is at $old"
    rm -rf "$new"
    exit 1
fi
rm -rf "$old" || echo "could not remove the old app at $old"
echo "installed Swell $version"
