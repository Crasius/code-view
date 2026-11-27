# Code View on macOS

Every release has a universal macOS app, cross-built on Linux (`docs/release.md`). The packaging
command below builds one on a Mac. It was last verified on an Apple-silicon Mac on 2026-10-19, with
an earlier script that used cargo-bundle instead of `scripts/bundle-macos.sh` (Packaging
verification). The pull request workflow tests the crates with macOS code on a Mac and packages
the universal app on a manual run. The **NOT-VERIFIED** list tracks remaining checks.

## Getting the source

The repository is public on GitHub; clone it with the address its page shows (Code, HTTPS). No
account is needed to clone or build. To contribute, see `CONTRIBUTING.md`.

## Trying it without building

A release's `.app` comes from Creative Downloads, as a zip archive (`README.md`, Download). For a
build of `main` between releases, a maintainer starts `.github/workflows/pull-request.yml` by hand;
no push starts it, and a pull request runs only its checks. The manual run's packaging job (after
the checks pass) uploads the universal app as the artifact `CodeView-macos-universal`.
Downloading a run's artifact needs a GitHub sign-in:

```sh
gh workflow run pull-request.yml --ref main                                          # maintainers only
gh run list --workflow pull-request.yml --branch main --status success --limit 1   # the run id
gh run download <run id> --name CodeView-macos-universal
ditto -x -k CodeView-macos-universal.zip .
xattr -dr com.apple.quarantine "Code View.app"   # needed after a browser download, harmless otherwise
open "Code View.app"
```

The bundle is signed ad hoc, so Gatekeeper refuses it while it carries the quarantine flag that a
browser download adds ("cannot be opened because the developer cannot be verified");
`xattr -dr` clears it for the bundle and everything inside. Right-click → Open works too, once per
app.

## Building on a Mac

You need Xcode's command-line tools (`xcode-select --install`) and Rust (rustup).

```sh
cargo run -p review-app --bin CodeView            # debug build, runs from the terminal
sh scripts/check.sh                               # the checks; --list names their commands
cargo test --workspace
```

The tests never touch the login Keychain: app data in folders a test picked (`AppPaths::new`)
keeps its tokens in its own connections file (`TokenVault::File`), the setup's check whether a
token could be stored asks that vault too, and the app's tests reach their mock servers on
127.0.0.1 only. A test binary that read Keychain items written by an earlier build would wait on
an access prompt, which is what made the whole-app tests hang after a rebuild. The Keychain's own
round trip runs on request: `cargo test -p review-store -- --ignored`. A whole-app test that
still waits on something that never returns stops the test binary with a message after five
minutes (`REVIEW_TEST_WATCHDOG_SECS` changes the limit).

## Build an installable app

From the repository root, run:

```sh
./scripts/package-macos.sh
```

This builds a release **Code View.app** with the existing icon, bundle identifier
`de.crasius.codeview`, version, Developer Tools category and macOS 12 minimum version.
It signs the app ad hoc, validates its metadata and signature, and creates:

- `dist/CodeView-macos-arm64.dmg` on Apple silicon, or
  `dist/CodeView-macos-x86_64.dmg` on Intel: open it and drag **Code View** onto
  **Applications**, then launch Code View from Applications or Spotlight.
- A `.zip` beside the disk image containing the same app.
- The app itself at `target/<Rust host>/release/bundle/Code View.app`.

No Rust toolchain is needed on the Mac where the app is installed. Git is still
needed for repository operations. The script fixes its build output to the repository's
`target` directory and replaces its generated disk image and archive on subsequent runs.
A plain `cargo build` still builds only the executable; use this script for installation.
The bundle itself (its `Info.plist`, the icon, the macOS minimum version) is written by
`scripts/bundle-macos.sh`, which the release build (`ci/build-release.sh`) calls too.

For a single app supporting both Apple silicon and Intel:

```sh
./scripts/package-macos.sh --universal
```

This installs both Rust targets, builds both architectures, and merges them before signing.
The outputs are `dist/CodeView-macos-universal.dmg` and `.zip`.
A manual run of the pull request workflow runs the same script and uploads both files after
the checks pass.

These packages are locally signed, **not notarized**. To distribute a downloadable app
without Gatekeeper warnings, use Developer ID signing and notarization as described below;
recreate the disk image and ZIP from the signed and stapled app before distributing them.

The icon: `cargo run -p review-app --example render_icon` renders `codeview.icns` (PNG entries
from 16 to 1024 px on Apple's 824/1024 grid) next to the Windows `.ico` and the PNGs, from the same
SVG artworks. macOS uses a solid rounded tile behind the mark. The renderer also writes
`codeview-macos-512.png` from the same image as the 512px ICNS entry: eframe sets the
application icon at startup, so this matching PNG is used for the Dock and Command-Tab
instead of the transparent 64px Windows icon. After updating, replace the installed app
and fully quit/relaunch it to load the new runtime icon.

## What is implemented

Everything that differs per system lives behind one `platform` module per crate; feature code
calls `platform::…` only.

| Area | macOS | Where |
|---|---|---|
| Access tokens | Login Keychain, one generic password per connection (service "Code View", account = connection id). The connections file keeps the marker `keychain`; removing a connection deletes its item. A connections file copied from Windows asks for the token again. | `crates/review-store/src/platform/macos.rs` |
| Data folders | `~/Library/Application Support/ReviewApp` (settings, connections), `~/Library/Caches/ReviewApp` (repositories, logs) | same |
| Browser extension | Host manifests `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/de.crasius.codeview.json` and `~/Library/Application Support/Microsoft Edge/NativeMessagingHosts/de.crasius.codeview.json`; the unpacked extension in `~/Library/Caches/ReviewApp/browser/extension/`; a later start hands its link over the socket `~/Library/Caches/ReviewApp/ipc/codeview.sock`, whose folder only the owner may enter (mode 0700). Details: [browser-extension.md](browser-extension.md) | `crates/cv-shell/src/browser/`, `crates/cv-shell/src/single_instance/platform/unix.rs`, `crates/review-store/src/paths.rs` |
| Housekeeping priority | Background quality of service (`pthread_set_qos_class_self_np`) | same |
| Git child processes | Nothing to hide (no console windows on macOS) | `crates/review-git/src/platform/other.rs` |
| Window chrome | Full-size content view with a transparent title bar: the native traffic lights stay, the app's bar is the title bar and centers the buttons in the actual bar height, including UI zoom, and reserves their measured width (none in full screen). Dragging the bar's free parts moves the window (`ViewportCommand::StartDrag`), a double click zooms it; the bar's widgets keep their clicks. App-drawn caption buttons exist on Windows only. | `crates/cv-base/src/platform/macos.rs`, `platform/window_drag.rs` |
| Fonts | San Francisco (`SFNS.ttf`, variable) for text, Helvetica as the fallback; SF Mono where installed, else Menlo, else Monaco for code; Apple Symbols. `.ttc` collections load by face index. Apple Color Emoji holds bitmaps only, which egui does not draw, so egui's bundled emoji font stays. | `platform/macos.rs` |
| Shortcuts | All shortcuts match on egui's `Modifiers::COMMAND` (⌘ on macOS). Labels come from `cv_base::platform::shortcut!`. | `platform/keys.rs` |
| Focus mode | ⌃⌘F instead of F11 (F11 is Show Desktop on macOS; ⌃⌘F is the system's own full-screen shortcut, which focus mode resembles). Esc leaves it as on Windows. | `platform/keys.rs` |
| Reduced motion | System Settings → Accessibility → Display → Reduce motion (`NSWorkspace`) | `platform/macos.rs` |
| Frame rate | The main screen's maximum refresh rate (`NSScreen.maximumFramesPerSecond`, 120 on ProMotion). The frame limit sleeps instead of waiting on input like on Windows. | `platform/macos.rs`, `platform/portable.rs` |
| Save dialog | The standard Save dialog through AppleScript's `choose file name`; a name typed without an extension gets `.log`. | `platform/macos.rs` |
| Show folder | `open <folder>` (Finder) | `platform/macos.rs` |
| `gh`, `az`, `git` | Apps started from the Dock get a minimal `PATH`; Code View adds `/opt/homebrew/bin`, `/usr/local/bin` and `/opt/local/bin` at start. | `platform/macos.rs` |
| Window placement | Not restored: the window opens centered at its default size (documented no-op). | `platform/portable.rs` |

### Shortcut mapping

| Action | Windows | macOS |
|---|---|---|
| Search pull requests | Ctrl+K | ⌘K |
| Focus mode | F11 | ⌃⌘F |
| Post a comment | Ctrl+Enter | ⌘↩ |
| Copy, select all | Ctrl+C, Ctrl+A | ⌘C, ⌘A |
| Copy the whole log | Ctrl+Shift+C | ⇧⌘C |
| Bold, italic, code, link | Ctrl+B, Ctrl+I, Ctrl+E, Ctrl+K | ⌘B, ⌘I, ⌘E, ⌘K |
| Strikethrough, heading, code block, quote | Ctrl+Shift+X, H, E, . | ⇧⌘X, H, E, . |
| Lists (bulleted, numbered, task) | Ctrl+Shift+8, 7, 9 | ⇧⌘8, 7, 9 |
| Insert suggestion | Ctrl+G | ⌘G |
| Zoom in, out, reset | Ctrl+=, Ctrl+-, Ctrl+0 | ⌘=, ⌘-, ⌘0 |
| Zoom with the wheel | Ctrl+wheel | ⌘-scroll (pinch works too) |
| Open a link in the browser | Ctrl+click | ⌘-click |
| File tree in focus mode | Ctrl+B | ⌘B |
| Mark viewed, wrap, whitespace, new only, threads | Alt+V, Alt+Z, Alt+W, Alt+N, Alt+T | ⌥V, ⌥Z, ⌥W, ⌥N, ⌥T |
| Comment on the selected lines | Alt+C | ⌥C |
| Previous, next file | Alt+↑, Alt+↓ (or K, J) | ⌥↑, ⌥↓ (or K, J) |
| Back | Alt+← | ⌥← |
| Frame statistics | Ctrl+Shift+F | ⇧⌘F |
| Next, previous change | N or F7, P or Shift+F7 | N or F7, P or ⇧F7 |
| Refresh | F5 | F5 (fn+F5 on a laptop keyboard) |

## Signing and notarization

CI signs the bundle ad hoc (`codesign --sign -`). That runs on the machine that built it; on any
other Mac, Gatekeeper refuses an ad-hoc app downloaded from the internet (right-click → Open, or
`xattr -dr com.apple.quarantine "Code View.app"`, gets past it for testing; see
[Trying it without building](#trying-it-without-building)).

To distribute, sign with a Developer ID Application certificate and notarize:

```sh
# Sign with the hardened runtime (required for notarization).
codesign --force --deep --options runtime --timestamp \
  --sign "Developer ID Application: <Name> (<TEAMID>)" "Code View.app"
codesign --verify --deep --strict --verbose=2 "Code View.app"

# Notarize: store the credentials once (app-specific password from appleid.apple.com),
# then submit a zip and wait for the verdict.
xcrun notarytool store-credentials codeview-notary --apple-id <apple id> --team-id <TEAMID>
ditto -c -k --keepParent "Code View.app" CodeView.zip
xcrun notarytool submit CodeView.zip --keychain-profile codeview-notary --wait

# Staple the ticket so Gatekeeper accepts the app offline, then check it.
xcrun stapler staple "Code View.app"
spctl --assess --type execute --verbose "Code View.app"
```

The Keychain ties the stored tokens to the app's signature: after switching from ad-hoc to
Developer ID signing (or between two ad-hoc builds), macOS asks once whether the new build may
read each token ("Always Allow").

## Packaging verification (2026-10-19)

On Apple silicon with Rust 1.99.1, with the earlier script that used cargo-bundle 0.12.0:

- Native and universal release packaging completed; disk-image checksums passed.
- Bundle metadata, embedded `.icns` and ad-hoc signature were checked.
- The native disk image mounted and contained the app and an Applications shortcut.
- The packaged native executable launched from the disk image and displayed the
  pull-request screen. Its dynamic dependencies were macOS system libraries only.

This is a packaging smoke test, not a pass of the full application test suite.
An Intel Mac and macOS 12 were not available for runtime testing.

## NOT-VERIFIED

Remaining behavior and compatibility checks:

- Keychain: storing, reading and deleting tokens; the access prompts after re-signing; the
  ignored `round_trips_through_the_keychain` test (`cargo test -p review-store -- --ignored`).
- Window chrome: native full-screen transitions, dragging and double-click zoom. Traffic-light
  alignment and unclipped button frames are checked by the native regression example below.
- Fonts: `SFNS.ttf` without an `opsz` setting, the Helvetica `.ttc` face indices (0 regular,
  1 bold), SF Mono's presence (it ships with Xcode and Terminal, not in `/System/Library/Fonts` on
  every release).
- ⌥ shortcuts while a text field has focus: ⌥V and friends type characters (√) there, as Alt does
  not on Windows.
- ⌃⌘F against a menu item that takes it (winit's default menu has none).
- The AppleScript Save dialog: whether it comes to the front over the app.
- The browser extension: registering the host manifests, Chrome or Edge starting the host inside
  the app bundle, and a second start handing its link over the socket.
- The bundle: Intel/macOS 12 runtime compatibility and icon appearance in the Dock and Finder.
- `scripts/package-macos.sh` with `scripts/bundle-macos.sh`, native and `--universal`, on a Mac.

## Traffic-light alignment regression check

```sh
cargo run --locked -p cv-base --example check_macos_traffic_lights
```

This creates an offscreen AppKit window on the main thread (no GPU or provider connection),
checks each native button's center and parent containment at 75%, 100% and 150% UI zoom,
and repeats the original geometry after resizing. The app reapplies alignment while drawing
the title bar because AppKit can reset it on resize. Button sizes, horizontal spacing and
native actions are preserved; native full-screen layout remains under AppKit's control.
