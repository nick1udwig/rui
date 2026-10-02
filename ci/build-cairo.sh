#!/usr/bin/env bash
set -euo pipefail

# Source and checksum published at:
# https://www.cairographics.org/news/cairo-1.18.4/
cairo_prefix=${1:?usage: build-cairo.sh ABSOLUTE_PREFIX SCRATCH_DIRECTORY}
cairo_scratch=${2:?usage: build-cairo.sh ABSOLUTE_PREFIX SCRATCH_DIRECTORY}
case "$cairo_prefix" in /*) ;; *) exit 2 ;; esac
mkdir -p "$cairo_scratch"
cairo_scratch=$(cd "$cairo_scratch" && pwd)
cairo_archive="$cairo_scratch/cairo-1.18.4.tar.xz"
if [[ ! -f "$cairo_archive" ]]; then
  curl --fail --location --retry 3 https://cairographics.org/releases/cairo-1.18.4.tar.xz --output "$cairo_archive"
fi
printf '%s  %s\n' 445ed8208a6e4823de1226a74ca319d3600e83f6369f99b14265006599c32ccb "$cairo_archive" | sha256sum --check --status
tar --extract --xz --file "$cairo_archive" --directory "$cairo_scratch" --no-same-owner
meson setup "$cairo_scratch/build" "$cairo_scratch/cairo-1.18.4" \
  --prefix "$cairo_prefix" --libdir lib --buildtype release \
  -Dauto_features=disabled -Dtests=disabled -Ddefault_library=shared
meson compile -C "$cairo_scratch/build" -j 2
meson install -C "$cairo_scratch/build"
