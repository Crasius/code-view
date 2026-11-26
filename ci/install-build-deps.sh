#!/bin/sh
# Unpacks the named tools into $BUILD_DEPS/usr/bin, each pinned by version and checksum:
# cargo-about writes the license notices of a release build, cargo-nextest runs the tests. The
# runner drops every capability, so nothing can be installed system-wide (docs/release.md, Runner).
# Usage: install-build-deps.sh cargo-about|cargo-nextest ...
set -eu
root="${BUILD_DEPS:?Name the folder to unpack into.}"
[ "$#" -gt 0 ] || { echo "Name the tools to install." >&2; exit 2; }
mkdir -p "$root/usr/bin"

# fetch <url> <sha256>: downloads into a fresh folder and prints the file's path.
fetch() {
    file="$(mktemp -d)/${1##*/}"
    curl --fail --silent --show-error --location --proto '=https' --output "$file" "$1"
    echo "$2  $file" | sha256sum -c - >&2
    printf '%s\n' "$file"
}

for tool in "$@"; do
    case "$tool" in
        cargo-about)
            name="cargo-about-0.9.3-x86_64-unknown-linux-musl"
            file="$(fetch "https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.3/$name.tar.gz" \
                3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4)"
            tar --no-same-owner -xzf "$file" -C "$root/usr/bin" --strip-components=1 "$name/cargo-about"
            ;;
        cargo-nextest)
            # The same version as .github/workflows/pull-request.yml installs.
            file="$(fetch "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.151/cargo-nextest-0.9.151-x86_64-unknown-linux-musl.tar.gz" \
                6b284d6fde9670dfb6ee10ea9d9abbc85f95643d3f592472d420d82e257b3a6c)"
            tar --no-same-owner -xzf "$file" -C "$root/usr/bin" cargo-nextest
            ;;
        *)
            echo "install-build-deps: unknown tool $tool" >&2
            exit 2
            ;;
    esac
done
