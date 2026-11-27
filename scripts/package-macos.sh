#!/bin/bash
# Build an installable app and drag-to-Applications disk image on macOS.
set -euo pipefail

usage() {
  echo "Usage: $0 [--universal]"
  echo "Builds a release .app, .dmg and .zip for this Mac (or both Mac architectures)."
}

universal=false
case "${1:-}" in
  "") ;;
  --universal) universal=true ;;
  -h|--help) usage; exit 0 ;;
  *) usage >&2; exit 2 ;;
esac
if (( $# > 1 )); then usage >&2; exit 2; fi
if [[ "$(uname -s)" != Darwin ]]; then
  echo "This packaging command must run on macOS." >&2
  exit 1
fi
for tool in cargo rustc codesign hdiutil ditto; do
  if ! command -v "$tool" >/dev/null; then
    echo "Missing $tool. Install Rust and the Xcode command-line tools." >&2
    exit 1
  fi
done

root="$(cd "$(dirname "$0")/.." && pwd)"
# A fixed output directory keeps local and CI artifact paths identical.
export CARGO_TARGET_DIR="$root/target"
host="$(rustc -vV | sed -n 's/^host: //p')"
case "$host" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "Unsupported Rust host: $host" >&2; exit 1 ;;
esac
MACOSX_DEPLOYMENT_TARGET="$(sh "$root/scripts/bundle-macos.sh" --deployment-target)"
export MACOSX_DEPLOYMENT_TARGET
cd "$root"
if "$universal"; then
  if ! command -v rustup >/dev/null; then
    echo "Universal builds require rustup to install both Rust targets." >&2
    exit 1
  fi
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --locked --release -p review-app --bin CodeView --target "$target"
  done
  arch=universal
fi

cargo build --locked --release -p review-app --bin CodeView --target "$host"
version="$(cargo pkgid -p review-app | sed 's/.*[#@]//')"
bundle="$CARGO_TARGET_DIR/$host/release/bundle"
sh "$root/scripts/bundle-macos.sh" "$bundle" "$CARGO_TARGET_DIR/$host/release/CodeView" "$version"
app="$bundle/Code View.app"
test -x "$app/Contents/MacOS/CodeView"
/usr/bin/plutil -lint "$app/Contents/Info.plist"
icon="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$app/Contents/Info.plist")"
test -s "$app/Contents/Resources/$icon"
if "$universal"; then
  lipo -create -output "$app/Contents/MacOS/CodeView" \
    "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/CodeView" \
    "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/CodeView"
  lipo "$app/Contents/MacOS/CodeView" -verify_arch arm64 x86_64
fi
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

mkdir -p "$root/dist"
staging="$(mktemp -d "$root/dist/.macos-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
# ditto preserves bundle permissions and symlinks.
ditto "$app" "$staging/Code View.app"
ln -s /Applications "$staging/Applications"
artifact="$root/dist/CodeView-macos-$arch"
hdiutil create -volname "Code View" -srcfolder "$staging" -format UDZO -ov "$artifact.dmg"
ditto -c -k --keepParent "$app" "$artifact.zip"
hdiutil verify "$artifact.dmg"
printf '\nApp: %s\nInstaller: %s.dmg\nArchive: %s.zip\n' "$app" "$artifact" "$artifact"
echo "Open the disk image and drag Code View into Applications."
