#!/bin/sh
# Writes the app bundle macOS launches, the one place that holds its Info.plist values: Finder and
# the Dock read the name and the icon from it, and the login Keychain ties saved tokens to the
# identifier (docs/macos.md). The release build (ci/build-release.sh) and the build on a Mac
# (scripts/package-macos.sh) both call it, so both ship the same Info.plist.
#   bundle-macos.sh <out dir> <binary> <version>   writes <out dir>/Code View.app
#   bundle-macos.sh --deployment-target            prints the oldest macOS the build targets, for
#                                                  MACOSX_DEPLOYMENT_TARGET before cargo builds
# POSIX sh; runs on Linux (the release cross-build) and macOS.
set -eu
deployment_target=12.0

if [ "${1:-}" = --deployment-target ]; then
    echo "$deployment_target"
    exit 0
fi
out="${1:?Name the folder for Code View.app.}"
binary="${2:?Name the CodeView binary.}"
version="${3:?Name the version.}"
root="$(cd "$(dirname "$0")/.." && pwd)"

contents="$out/Code View.app/Contents"
rm -rf "$out/Code View.app"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$binary" "$contents/MacOS/CodeView"
cp "$root/crates/review-app/assets/icon/codeview.icns" "$contents/Resources/codeview.icns"
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
  <key>LSMinimumSystemVersion</key><string>$deployment_target</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright 2026 Northbyte. Functional Source License 1.1 (FSL-1.1-ALv2).</string>
</dict>
</plist>
EOF
