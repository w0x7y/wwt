#!/bin/sh
set -eu

project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
prefix=/opt/wwt-test
case ${CARGO_TARGET_DIR:-target} in
    /*) build_dir=${CARGO_TARGET_DIR:-target} ;;
    *) build_dir=$project/${CARGO_TARGET_DIR:-target} ;;
esac

make -C "$project" build
make -C "$project" install DESTDIR="$stage" PREFIX="$prefix" CARGO=false

test -x "$stage$prefix/bin/wwt"
test -f "$stage$prefix/share/applications/wwt.desktop"
desktop-file-validate "$stage$prefix/share/applications/wwt.desktop"
grep -Fqx 'Exec=wwt --launch %u' "$stage$prefix/share/applications/wwt.desktop"
grep -Fqx 'Terminal=false' "$stage$prefix/share/applications/wwt.desktop"
test "$("$stage$prefix/bin/wwt" --version)" = "$("$build_dir/release/wwt" --version)"

icon_source="$stage/user-created-wwt.svg"
printf '<svg xmlns="http://www.w3.org/2000/svg"/>\n' >"$icon_source"
make -C "$project" install \
    DESTDIR="$stage" \
    PREFIX="$prefix" \
    CARGO=false \
    ICON_SOURCE="$icon_source"
test -f "$stage$prefix/share/icons/hicolor/scalable/apps/wwt.svg"

make -C "$project" uninstall DESTDIR="$stage" PREFIX="$prefix"

test ! -e "$stage$prefix/bin/wwt"
test ! -e "$stage$prefix/share/applications/wwt.desktop"
test ! -e "$stage$prefix/share/icons/hicolor/scalable/apps/wwt.svg"

custom_target="$stage/custom-target"
mkdir -p "$custom_target/release"
printf '#!/bin/sh\nprintf "custom target binary\\n"\n' >"$custom_target/release/wwt"
chmod +x "$custom_target/release/wwt"

make -C "$project" install \
    DESTDIR="$stage" \
    PREFIX="$prefix" \
    CARGO=false \
    CARGO_TARGET_DIR="$custom_target"
test "$("$stage$prefix/bin/wwt")" = "custom target binary"

make -C "$project" uninstall DESTDIR="$stage" PREFIX="$prefix"
