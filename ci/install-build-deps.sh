#!/bin/sh
# Unpacks cargo-about, which writes the license notices of a release build, into $BUILD_DEPS. The
# runner drops every capability, so nothing can be installed system-wide (docs/release.md, Runner).
set -eu
root="${BUILD_DEPS:?Name the folder to unpack into.}"

cargo_about="cargo-about-0.9.3-x86_64-unknown-linux-musl"
download="$(mktemp -d)/$cargo_about.tar.gz"
curl --fail --silent --show-error --location --proto '=https' --output "$download" \
    "https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.3/$cargo_about.tar.gz"
echo "3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4  $download" | sha256sum -c -
mkdir -p "$root/usr/bin"
tar --no-same-owner -xzf "$download" -C "$root/usr/bin" --strip-components=1 "$cargo_about/cargo-about"
