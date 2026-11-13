#!/usr/bin/env bash
# Bygger Kladd-<version>-x86_64.AppImage med linuxdeploy. Kör från repots rot:
#   packaging/linux/build-appimage.sh <version> [binär] [verktygskatalog]
# Binären är som standard target/release/kladd. linuxdeploy och dess AppImage-plugin
# laddas ner till verktygskatalogen om de saknas. Resultatet hamnar i dist/.
set -euo pipefail

version="${1:?ange version, t.ex. 1.9.0}"
bin="$(realpath "${2:-target/release/kladd}")"
tools="${3:-$PWD/dist/linuxdeploy}"

mkdir -p "$tools" dist
base=https://github.com/linuxdeploy
[ -x "$tools/linuxdeploy" ] || {
  curl -fsSL -o "$tools/linuxdeploy" "$base/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
  chmod +x "$tools/linuxdeploy"
}
[ -x "$tools/linuxdeploy-plugin-appimage" ] || {
  curl -fsSL -o "$tools/linuxdeploy-plugin-appimage" "$base/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage"
  chmod +x "$tools/linuxdeploy-plugin-appimage"
}

# FUSE finns inte på körmiljön: AppImage-verktygen packar upp sig själva och körs därifrån.
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
export LINUXDEPLOY_OUTPUT_VERSION="$version"
# Pluginet hittas via PATH.
export PATH="$tools:$PATH"

appdir="$PWD/dist/Kladd.AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/share/doc/kladd"
cp assets/fonts/OFL.txt "$appdir/usr/share/doc/kladd/FONT-LICENSE"
cp assets/fonts/Carlito-OFL.txt "$appdir/usr/share/doc/kladd/CARLITO-FONT-LICENSE"
cp assets/fonts/STIXTwo-OFL.txt "$appdir/usr/share/doc/kladd/STIXTWO-FONT-LICENSE"
cp LICENSE README.md "$appdir/usr/share/doc/kladd/"

# linuxdeploy tar med delade bibliotek utom dem på sin standardlista över systembibliotek
# (libGL, libEGL, libwayland, libX*, libxkbcommon m.fl.): grafik och fönstersystem ska
# komma från värdens drivrutiner, inte från paketet.
# AppImagen skrivs till den aktuella katalogen, därför körs verktyget från dist/.
root="$PWD"
cd dist
"$tools/linuxdeploy" \
  --appdir "$appdir" \
  --executable "$bin" \
  --desktop-file "$root/packaging/kladd.desktop" \
  --icon-file "$root/packaging/icons/kladd.svg" \
  --icon-file "$root/packaging/icons/48/kladd.png" \
  --icon-file "$root/packaging/icons/128/kladd.png" \
  --icon-file "$root/packaging/icons/256/kladd.png" \
  --output appimage

ls -l Kladd-*.AppImage
