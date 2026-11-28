#!/bin/sh
# The one place that pins the build tools beyond the toolchain, each by version and by the SHA-256
# of every archive it is taken from: cargo-about writes the license notices of a release build,
# cargo-nextest runs the tests, cargo-deny checks the dependency graph and cargo-shear finds unused
# dependencies.
#
#   sh ci/install-build-deps.sh <tool>...    download, check and unpack the named tools
#   sh ci/install-build-deps.sh --path <tool>     the executable scripts/check.sh uses; exit 1 and
#                                                 the reason on stdout when there is none
#   sh ci/install-build-deps.sh --version <tool>  the pinned version
#
# With CI set (a runner) the tools go to $BUILD_DEPS/usr/bin, which the jobs put on PATH: the runner
# drops every capability, so nothing can be installed system-wide (docs/release.md, Runner), and
# --path names the tool on PATH. Without CI (a development machine) each tool goes to its own
# folder <tools>/<tool>-<version>/bin, shared by every checkout and worktree and never ~/.cargo/bin,
# so a tool installed there by hand stays as it is. <tools> is $CODE_VIEW_TOOLS, else the user's
# cache folder: %LOCALAPPDATA%\code-view\tools (Windows), ~/Library/Caches/code-view/tools (macOS),
# ${XDG_CACHE_HOME:-~/.cache}/code-view/tools (Linux). --path takes that folder's copy, else one on
# PATH whose version is the pinned one, never another version.
# Hosts: Windows x64 (Git Bash), macOS (Apple silicon; Intel where the project publishes a binary),
# Linux x64 and arm64. A host without a published archive gets the cargo install command that puts
# the tool into the same folder.
# Usage: install-build-deps.sh cargo-about|cargo-nextest|cargo-deny|cargo-shear ...
set -eu

case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) os=windows ;;
    Darwin) os=macos ;;
    *) os=linux ;;
esac
case "$(uname -m)" in
    aarch64 | arm64) arch=aarch64 ;;
    *) arch=x86_64 ;;
esac
case "$os" in
    windows) triple="$arch-pc-windows-msvc"; exe=.exe ;;
    macos) triple="$arch-apple-darwin"; exe="" ;;
    *) triple="$arch-unknown-linux-musl"; exe="" ;;
esac

# pin <tool>: sets version, and url, sha (empty when the project publishes no archive for this
# host), member (the executable's path in the archive) and strip (its leading folders).
pin() {
    sha=""
    case "$1" in
        cargo-about)
            version=0.9.3
            name="cargo-about-$version-$triple"
            url="https://github.com/EmbarkStudios/cargo-about/releases/download/$version/$name.tar.gz"
            member="$name/cargo-about$exe"
            strip=1
            case "$triple" in
                x86_64-unknown-linux-musl) sha=3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4 ;;
                aarch64-unknown-linux-musl) sha=eb4d9ff6ebd2518152a8308729431a04b3836e71d2bd5ead7aea69f91a71162e ;;
                aarch64-apple-darwin) sha=45520df8680978a6e7c4023ace0e3f311c73efc208cd68e5a7f4ed4822fa0349 ;;
                x86_64-pc-windows-msvc) sha=9e2942533ef1c592c4a70bdb384cae1550a3f5fc9ae40379b6662e3d2f16fe02 ;;
            esac
            ;;
        cargo-deny)
            version=0.22.1
            name="cargo-deny-$version-$triple"
            url="https://github.com/EmbarkStudios/cargo-deny/releases/download/$version/$name.tar.gz"
            member="$name/cargo-deny$exe"
            strip=1
            case "$triple" in
                x86_64-unknown-linux-musl) sha=13f7e825e9cb811c2f8de63f29b0f45851dfb53004029f0f2501af524728364d ;;
                aarch64-unknown-linux-musl) sha=0bac0838a1653ce693e4602b8634d15ca179edd2198e438af9c35ab1c551d509 ;;
                aarch64-apple-darwin) sha=2afa2d672d2b2ee012835528d0c2acb73693ee973a8dc377db0e09d7d8dee9a2 ;;
                x86_64-pc-windows-msvc) sha=b8d7e473f54c328cc528ef8aea221c40b5ccac3d63844aa056f86ed0886800a2 ;;
            esac
            ;;
        cargo-nextest)
            # Both CIs: GitLab's Linux jobs and GitHub's Linux and macOS jobs.
            version=0.9.151
            target="$triple"
            case "$os" in macos) target=universal-apple-darwin ;; esac
            url="https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$version/cargo-nextest-$version-$target.tar.gz"
            member="cargo-nextest$exe"
            strip=0
            case "$target" in
                x86_64-unknown-linux-musl) sha=6b284d6fde9670dfb6ee10ea9d9abbc85f95643d3f592472d420d82e257b3a6c ;;
                aarch64-unknown-linux-musl) sha=a9ee2d53cf0711d879f068c0ff814b6fe3af3f9be8799f8ab484e47571087160 ;;
                universal-apple-darwin) sha=d1d7d025d4bfc9fb154cd76aea3949bd78534829a0589abefa6e5dd4edb135dd ;;
                x86_64-pc-windows-msvc) sha=e51bdb4b5883df48f335381b287378edb244318fac65ae0adaf6b9e69298246b ;;
            esac
            ;;
        cargo-shear)
            # The project publishes no checksum file; these are the digests GitHub lists per asset.
            version=1.14.1
            target="$triple"
            case "$os" in macos) target=universal-apple-darwin ;; esac
            url="https://github.com/Boshen/cargo-shear/releases/download/v$version/cargo-shear-$target.tar.gz"
            member="cargo-shear$exe"
            strip=0
            case "$target" in
                x86_64-unknown-linux-musl) sha=7b4cec5ec12e22717670721ace1edbd97a65cb9e08864466d292a3009bb85d1a ;;
                aarch64-unknown-linux-musl) sha=7b8c61cca4f2628cbc847ba4dfda37924b8dd2dba8ba19fe4107797f87e6b3ea ;;
                universal-apple-darwin) sha=380b8da0da903340d17058ea6d9027ef5b64a9d6977564999f3066644e7593b8 ;;
                x86_64-pc-windows-msvc) sha=2e87bf0f9fa80c66ded35864ae288d2568ecc8c976bbbca98cfaea34737a194d ;;
            esac
            ;;
        *)
            echo "install-build-deps: unknown tool $1" >&2
            exit 2
            ;;
    esac
}

# The folder of a development machine's tools, as a POSIX path (it goes on PATH).
tools_dir() {
    dir="${CODE_VIEW_TOOLS:-}"
    if [ -z "$dir" ]; then
        case "$os" in
            windows) dir="${LOCALAPPDATA:?LOCALAPPDATA is not set}/code-view/tools" ;;
            macos) dir="$HOME/Library/Caches/code-view/tools" ;;
            *) dir="${XDG_CACHE_HOME:-$HOME/.cache}/code-view/tools" ;;
        esac
    fi
    if [ "$os" = windows ] && command -v cygpath > /dev/null; then
        dir="$(cygpath -u "$dir")"
    fi
    printf '%s\n' "$dir"
}

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

# unpack <tool> <folder>: the pinned executable of <tool> into <folder>.
unpack() {
    if [ -z "$sha" ]; then
        echo "install-build-deps: $1 $version publishes no binary for $triple; build it into the same folder with" >&2
        echo "  cargo install --locked --root \"$(tools_dir)/$1-$version\" $1@$version" >&2
        exit 1
    fi
    file="$(fetch "$url" "$sha")"
    mkdir -p "$2"
    tar --no-same-owner -xzf "$file" -C "$2" --strip-components="$strip" "$member"
    rm -rf "${file%/*}"
}

case "${1:-}" in
    --version)
        [ "$#" -eq 2 ] || { echo "usage: install-build-deps.sh --version <tool>" >&2; exit 2; }
        pin "$2"
        printf '%s\n' "$version"
        exit 0
        ;;
    --path)
        [ "$#" -eq 2 ] || { echo "usage: install-build-deps.sh --path <tool>" >&2; exit 2; }
        tool="$2"
        pin "$tool"
        if [ -n "${CI:-}" ]; then
            command -v "$tool" || { echo "$tool is not installed"; exit 1; }
            exit 0
        fi
        own="$(tools_dir)/$tool-$version/bin/$tool$exe"
        if [ -x "$own" ]; then
            printf '%s\n' "$own"
            exit 0
        fi
        found="$(command -v "$tool" || true)"
        if [ -z "$found" ]; then
            echo "$tool $version is not installed"
            exit 1
        fi
        # A cargo plugin takes its subcommand's name first, as cargo passes it.
        seen="$("$found" "${tool#cargo-}" --version 2> /dev/null | head -n 1 || true)"
        case " $seen " in
            *" $version "*) printf '%s\n' "$found"; exit 0 ;;
        esac
        echo "$tool on PATH (${seen:-version unknown}) is not the pinned $version"
        exit 1
        ;;
    '')
        echo "Name the tools to install." >&2
        exit 2
        ;;
esac

if [ -n "${CI:-}" ]; then
    root="${BUILD_DEPS:?Name the folder to unpack into.}"
    mkdir -p "$root/usr/bin"
    for tool in "$@"; do
        pin "$tool"
        unpack "$tool" "$root/usr/bin"
    done
    exit 0
fi

dir="$(tools_dir)"
for tool in "$@"; do
    pin "$tool"
    final="$dir/$tool-$version"
    if [ -x "$final/bin/$tool$exe" ]; then
        echo "install-build-deps: $tool $version is in $final/bin"
        continue
    fi
    # Unpacked beside the final folder and moved in whole, so an interrupted run leaves no half tool.
    rm -rf "$final.partial"
    unpack "$tool" "$final.partial/bin"
    rm -rf "$final"
    mv "$final.partial" "$final"
    echo "install-build-deps: installed $tool $version into $final/bin"
done
