#!/usr/bin/env bash
# Usage: appimage.sh BINARY VERSION OUTDIR
# Wraps BINARY (the static musl build) in OUTDIR/mqtt-zapp-VERSION-x86_64.AppImage.
# appimagetool and the type-2 runtime come from pinned releases, checked by
# sha256 and cached. Needs no FUSE: appimagetool runs extracted.
set -euo pipefail
bin=${1:?usage: appimage.sh BINARY VERSION OUTDIR}
version=${2:?usage: appimage.sh BINARY VERSION OUTDIR}
out=${3:?usage: appimage.sh BINARY VERSION OUTDIR}
here=$(cd "$(dirname "$0")" && pwd)

TOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
TOOL_SHA=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
RUNTIME_SHA=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

cache=${APPIMAGE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/mqtt-zapp-appimage}
mkdir -p "$cache"
fetch() { # URL SHA FILE
    if ! echo "$2  $cache/$3" | sha256sum -c --status 2>/dev/null; then
        curl -fsSL -o "$cache/$3.part" "$1"
        echo "$2  $cache/$3.part" | sha256sum -c --quiet
        mv "$cache/$3.part" "$cache/$3"
    fi
}
fetch "$TOOL_URL" "$TOOL_SHA" appimagetool
fetch "$RUNTIME_URL" "$RUNTIME_SHA" runtime
chmod +x "$cache/appimagetool"

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp:?}"' EXIT
app=$tmp/mqtt-zapp.AppDir
install -Dm755 "$bin" "$app/usr/bin/mqtt-zapp"
install -Dm644 "$here/mqtt-zapp.desktop" "$app/usr/share/applications/mqtt-zapp.desktop"
install -Dm644 "$here/mqtt-zapp.svg" "$app/usr/share/icons/hicolor/scalable/apps/mqtt-zapp.svg"
cp "$here/mqtt-zapp.desktop" "$here/mqtt-zapp.svg" "$app/"
ln -s usr/bin/mqtt-zapp "$app/AppRun"

mkdir -p "$out"
ARCH=x86_64 VERSION=$version APPIMAGE_EXTRACT_AND_RUN=1 \
    "$cache/appimagetool" --no-appstream --runtime-file "$cache/runtime" \
    "$app" "$out/mqtt-zapp-$version-x86_64.AppImage"
