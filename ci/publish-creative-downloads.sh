#!/bin/sh
# Publishes dist/<platform>/ as one Creative Downloads release with a depot per platform.
# CI variables: CDL_TOKEN (masked, protected; scopes creations:read and packages:publish),
# CDL_CREATION and CDL_VARIANT (protected).
# Usage: publish-creative-downloads.sh <notes file>
set -eu
notes="${1:?Name the release notes file.}"
: "${CDL_TOKEN:?Set the masked, protected CI variable CDL_TOKEN.}"
: "${CDL_CREATION:?Set the CI variable CDL_CREATION.}"
: "${CDL_VARIANT:?Set the CI variable CDL_VARIANT.}"

for file in linux-x64/CodeView linux-arm64/CodeView windows-x64/CodeView.exe windows-arm64/CodeView.exe \
    "macos/Code View.app/Contents/MacOS/CodeView" \
    linux-x64/THIRD-PARTY-NOTICES.txt linux-arm64/THIRD-PARTY-NOTICES.txt \
    windows-x64/THIRD-PARTY-NOTICES.txt windows-arm64/THIRD-PARTY-NOTICES.txt \
    "macos/Code View.app/Contents/Resources/THIRD-PARTY-NOTICES.txt"; do
    [ -f "dist/$file" ] || {
        echo "dist/$file is missing." >&2
        exit 1
    }
done

# Creative Downloads takes one macOS depot without an architecture; it carries the universal build.
cdl -o json publish "$CDL_CREATION" --variant "$CDL_VARIANT" \
    --depot "CodeView:windows:x64=dist/windows-x64" \
    --depot "CodeView:windows:arm64=dist/windows-arm64" \
    --depot "CodeView:linux:x64=dist/linux-x64" \
    --depot "CodeView:linux:arm64=dist/linux-arm64" \
    --depot "CodeView:mac=dist/macos" \
    --notes-file "$notes" --cancel-running > publish.json
cat publish.json
