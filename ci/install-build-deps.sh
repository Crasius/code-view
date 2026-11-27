#!/bin/sh
# Unpacks the named tools into $BUILD_DEPS/usr/bin, each pinned by version and checksum:
# cargo-about writes the license notices of a release build, cargo-nextest runs the tests,
# cargo-deny checks the dependency graph and cargo-shear finds unused dependencies. The
# runner drops every capability, so nothing can be installed system-wide (docs/release.md, Runner).
# Usage: install-build-deps.sh cargo-about|cargo-nextest|cargo-deny|cargo-shear ...
set -eu
root="${BUILD_DEPS:?Name the folder to unpack into.}"
[ "$#" -gt 0 ] || { echo "Name the tools to install." >&2; exit 2; }
mkdir -p "$root/usr/bin"

# fetch <url> <sha256>: downloads into a fresh folder and prints the file's path.
fetch() {
    file="$(mktemp -d)/${1##*/}"
    curl --fail --silent --show-error --location --proto '=https' --output "$file" "$1"
    if command -v sha256sum > /dev/null; then
        echo "$2  $file" | sha256sum -c - >&2
    else
        echo "$2  $file" | shasum -a 256 -c - >&2
    fi
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
            # Both CIs: GitLab's Linux jobs and GitHub's Linux and macOS jobs.
            release="https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.151"
            case "$(uname -s)" in
                Darwin)
                    file="$(fetch "$release/cargo-nextest-0.9.151-universal-apple-darwin.tar.gz" \
                        d1d7d025d4bfc9fb154cd76aea3949bd78534829a0589abefa6e5dd4edb135dd)"
                    ;;
                *)
                    file="$(fetch "$release/cargo-nextest-0.9.151-x86_64-unknown-linux-musl.tar.gz" \
                        6b284d6fde9670dfb6ee10ea9d9abbc85f95643d3f592472d420d82e257b3a6c)"
                    ;;
            esac
            tar --no-same-owner -xzf "$file" -C "$root/usr/bin" cargo-nextest
            ;;
        cargo-deny)
            # scripts/check.sh reads the version= lines of this entry and the next one.
            version=0.22.1
            name="cargo-deny-$version-x86_64-unknown-linux-musl"
            file="$(fetch "https://github.com/EmbarkStudios/cargo-deny/releases/download/$version/$name.tar.gz" \
                13f7e825e9cb811c2f8de63f29b0f45851dfb53004029f0f2501af524728364d)"
            tar --no-same-owner -xzf "$file" -C "$root/usr/bin" --strip-components=1 "$name/cargo-deny"
            ;;
        cargo-shear)
            version=1.14.1
            file="$(fetch "https://github.com/Boshen/cargo-shear/releases/download/v$version/cargo-shear-x86_64-unknown-linux-musl.tar.gz" \
                7b4cec5ec12e22717670721ace1edbd97a65cb9e08864466d292a3009bb85d1a)"
            tar --no-same-owner -xzf "$file" -C "$root/usr/bin" cargo-shear
            ;;
        *)
            echo "install-build-deps: unknown tool $tool" >&2
            exit 2
            ;;
    esac
done
