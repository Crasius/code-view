#!/bin/sh
# Cross-builds the release binary of one platform on the Linux runner into dist/<platform>/, the
# folder ci/publish-creative-downloads.sh publishes. docs/release.md says why each platform is built
# this way. Needs the image of its job (.gitlab-ci.yml) and ci/install-build-deps.sh.
# Usage: build-release.sh <linux-x64|linux-arm64|macos|windows-x64|windows-arm64> [<tag>]
# A tag vX.Y.Z must match the version of Cargo.toml.
set -eu
platform="${1:?Name the platform: linux-x64, linux-arm64, macos, windows-x64 or windows-arm64.}"
tag="${2:-}"
target_dir="${CARGO_TARGET_DIR:-target}"

rustup show active-toolchain || rustup toolchain install

version="$(cargo pkgid -p review-app | sed 's/.*[#@]//')"
if [ -n "$tag" ] && [ "$tag" != "v$version" ]; then
    echo "The tag $tag does not match the version $version in Cargo.toml." >&2
    exit 1
fi

# The oldest glibc the Linux builds run on: Debian 11 and Ubuntu 20.04.
glibc=2.31

# Writes the license notices of what links on the given targets (docs/release.md, Third-party notices).
# Usage: write_notices <file> --target <triple> [--target <triple>]
write_notices() {
    notices="$1"
    shift
    cargo-about generate --locked --config ci/about.toml --manifest-path crates/review-app/Cargo.toml "$@" \
        -o "$notices" ci/about.hbs
    {
        printf '\n================================================================================\n'
        printf 'The Phosphor icon fonts, built into the program\n\n'
        cat crates/cv-base/assets/Phosphor-LICENSE-MIT.txt
        printf '\n================================================================================\n'
        printf 'The C# syntax grammar, built into the program\n\n'
        cat crates/review-grammars/grammars/csharp-license.txt
        printf '\n================================================================================\n'
        printf 'The syntax grammars of the bat project, built into the program through two-face\n'
        printf '(https://github.com/sharkdp/bat, https://codeberg.org/CosmicHarper/two-face)\n\n'
        env -u RUSTFLAGS cargo run --quiet --locked -p review-grammars --example grammar_notices
    } >> "$notices"
}

# Writes the app bundle macOS launches, with the values of [package.metadata.bundle] in
# crates/review-app/Cargo.toml: Finder and the Dock read its name and icon from Info.plist.
bundle_macos() {
    contents="$1/Code View.app/Contents"
    mkdir -p "$contents/MacOS" "$contents/Resources"
    cp "$2" "$contents/MacOS/CodeView"
    cp crates/review-app/assets/icon/codeview.icns "$contents/Resources/codeview.icns"
    cat > "$contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>Code View</string>
  <key>CFBundleExecutable</key><string>CodeView</string>
  <key>CFBundleIconFile</key><string>codeview</string>
  <key>CFBundleIdentifier</key><string>de.crasius.codeview</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Code View</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright 2026 Northbyte. Functional Source License 1.1 (FSL-1.1-ALv2).</string>
</dict>
</plist>
EOF
}

# A static C runtime: a fresh Windows need not have the Visual C++ redistributable. Its libraries name
# debug files that only Microsoft has (LNK4099), and the release build strips debug info. The flags go
# through the target's variable: cargo-xwin sets that one, and RUSTFLAGS would replace it.
build_windows() {
    rustup target add "$1"
    export "CARGO_TARGET_$(echo "$1" | tr 'a-z-' 'A-Z_')_RUSTFLAGS=-C target-feature=+crt-static -C link-arg=/IGNORE:4099"
    cargo xwin build --locked --release -p review-app --bin CodeView --target "$1"
    cp "$target_dir/$1/release/CodeView.exe" "$output/CodeView.exe"
    write_notices "$output/THIRD-PARTY-NOTICES.txt" --target "$1"
}

build_linux() {
    rustup target add "$1"
    cargo zigbuild --locked --release -p review-app --bin CodeView --target "$1.$glibc"
    cp "$target_dir/$1/release/CodeView" "$output/CodeView"
    write_notices "$output/THIRD-PARTY-NOTICES.txt" --target "$1"
}

output="dist/$platform"
rm -rf "$output"
mkdir -p "$output"

case "$platform" in
    linux-x64) build_linux x86_64-unknown-linux-gnu ;;
    linux-arm64) build_linux aarch64-unknown-linux-gnu ;;
    macos)
        # Creative Downloads takes one macOS depot without an architecture, so one universal binary
        # serves Apple silicon and Intel. The image brings the SDK (SDKROOT).
        export MACOSX_DEPLOYMENT_TARGET=12.0
        rustup target add aarch64-apple-darwin x86_64-apple-darwin
        cargo zigbuild --locked --release -p review-app --bin CodeView --target universal2-apple-darwin
        bundle_macos "$output" "$target_dir/universal2-apple-darwin/release/CodeView"
        write_notices "$output/Code View.app/Contents/Resources/THIRD-PARTY-NOTICES.txt" \
            --target aarch64-apple-darwin --target x86_64-apple-darwin
        ;;
    windows-x64) build_windows x86_64-pc-windows-msvc ;;
    windows-arm64) build_windows aarch64-pc-windows-msvc ;;
    *)
        echo "Unknown platform: $platform" >&2
        exit 2
        ;;
esac

find "$output" -type f -exec ls -l {} +
