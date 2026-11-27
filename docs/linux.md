# Code View on Linux

Every release has builds for Linux x64 and arm64, for glibc 2.31 or newer (`docs/release.md`). The
workspace lints clean and its tests pass on Debian 12 in a container
([Tests in a container](#tests-in-a-container)), the store and the app have Linux files of their
own, and a first run on an Ubuntu-like GNOME desktop found the problems this page now answers. The
**NOT-VERIFIED** list at the end names what still needs a Linux desktop.

## Building

You need Rust (rustup) and a C compiler; nothing else from the system at build time:

```sh
sudo apt install build-essential        # Fedora: sudo dnf install gcc; Arch: sudo pacman -S base-devel
cargo run -p review-app --bin CodeView   # debug build, runs from the terminal
sh scripts/check.sh                      # the checks; --list names their commands
cargo test --workspace
```

- **TLS without OpenSSL.** On Linux, HTTPS goes through rustls with the aws-lc-rs crypto provider
  and the platform certificate verifier (the system's CA store, so company CAs installed with
  `update-ca-certificates` are trusted). `libssl-dev` and `pkg-config` are not needed. aws-lc-sys
  compiles its C sources with the C compiler above. Windows (SChannel) and macOS (Secure Transport)
  keep the system TLS through native-tls; see `crates/review-core/Cargo.toml`.
- **Secret Service without libdbus.** Tokens go over D-Bus with zbus (pure Rust) and RustCrypto;
  `libdbus-1-dev` is not needed.
- **Windowing.** winit's X11 and Wayland backends load `libX11`, `libxkbcommon`, `libwayland-client`
  and `libEGL`/`libGL` at run time; no `-dev` packages are needed to build.

## Runtime dependencies

| What | Why | Package (Debian/Ubuntu) |
|---|---|---|
| OpenGL 3.3 or GLES 3 driver (Mesa or the vendor's) | glow renderer | `libgl1`, `libegl1` (installed with any desktop) |
| X11 or a Wayland compositor | the window | — |
| `libxkbcommon` | keyboard layout | `libxkbcommon0`, `libxkbcommon-x11-0` |
| A Secret Service: GNOME Keyring, KWallet (5.97+) or KeePassXC | access tokens | `gnome-keyring` |
| `git` (2.39.5 and 2.47.3 tested) | mirrors and diffs | `git` |
| `fontconfig` (`fc-match`), optional | the desktop's own fonts | `fontconfig` |
| `gsettings`, optional | GNOME's font and animation settings | `libglib2.0-bin` |
| `xdg-open`, optional | "Open folder" | `xdg-utils` |

## Tests in a container

The workspace's checks run in the official Rust image with the pinned toolchain and nothing
installed on top: Debian 12's git 2.39.5 and its six DejaVu fonts are all the tests need. Take the
image `RUST_IMAGE` names in `ci/checks.yml` (the CI's test image, pinned by digest); `<RUST_IMAGE>`
below stands for it. From the repository root (Git Bash on Windows needs `MSYS_NO_PATHCONV=1` in
front):

```sh
docker run --rm --cpus 6 -v "$PWD:/src:ro" -v codeview-linux-target:/target   -v codeview-linux-cargo:/usr/local/cargo/registry <RUST_IMAGE> bash -c '
  cd /src && export CARGO_TARGET_DIR=/target RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER=
  cargo clippy -j 6 --workspace --all-targets -- -D warnings &&
  cargo test -j 6 --workspace --no-fail-fast'
```

The two volumes keep the build and the crates between runs; the empty `RUSTC_WRAPPER` turns off
the Windows sccache that `.cargo/config.toml` names. No test reaches for a GPU, a display, a
keyring or the network. What depends on the machine is kept out of the tests:

- **Fonts.** The app takes the desktop's fonts; tests that assert what fits beside what in a row
  switch to the bundled fonts (`cv_testkit::use_bundled_fonts`), since DejaVu Sans runs wider
  than Segoe UI. The clipping audit keeps the machine's fonts on purpose: every screen must stay
  whole with any of them.
- **Tools on `PATH`.** The setup's GitHub CLI and Azure CLI options are set by the tests before
  the first frame, so an installed or missing `gh` or `az` changes nothing.
- **Speed.** A local `git diff` lands within a frame on Linux; tests that tell a cached diff from a
  loaded one ask the diff cache instead of counting frames.
- **git versions.** Which blobs a mirror lacks is asked with `git rev-list --missing=print`:
  git 2.39.5's `cat-file --batch-check` stops at the first missing blob instead of naming it
  (`crates/review-git/src/line_counts.rs`), and `ls-tree -l` stops the same way, so a file's
  entry is read without its size (`crates/review-git/src/mirror.rs`).

## Secret Service

Code View stores each connection's token as one item in the Secret Service, like the macOS
Keychain items: attributes `service` = `Code View` and `account` = the connection's id (`gh-1`,
`ado-2`, …), label `Code View: <id>`. The connections file (`~/.config/ReviewApp/connections.json`)
keeps only the marker `secret-service`. Removing a connection deletes its item. A connections file
copied from Windows or macOS asks for each token again.

When no Secret Service answers (no keyring daemon, a headless or minimal session), the setup says so
on the sign-in step and again at **Save connection**, with the install command for your
distribution, a **Copy commands** button, the post "Getting started with Code View" on Creative
Downloads, **Check again** and **Save without the token**
(anonymous connections browse public pull requests). To fix it:

```sh
sudo apt install gnome-keyring                       # or dnf / pacman / zypper
gnome-keyring-daemon --start --components=secrets    # or sign out and back in
```

KeePassXC: Tools → Settings → Secret Service Integration → Enable, and expose a database group.
KWallet: KDE Frameworks 5.97 or later provide `org.freedesktop.secrets`.

Check what is stored, from a terminal:

```sh
secret-tool search --all service "Code View"          # package libsecret-tools; prints the items
secret-tool clear service "Code View" account gh-1    # deletes one
```

If the keyring is locked, the desktop asks for its password when Code View saves or reads a token.
If the keyring is unavailable at start-up, the saved connections cannot be read; Code View says so
in a message and starts without them (the file stays untouched), so start the keyring and restart.

The app's tests never touch your keyring and run on machines without one: app data in folders a
test picked (`AppPaths::new`) keeps its tokens in its own connections file (`TokenVault::File`),
and the setup's keyring check asks that vault instead of D-Bus. The setup's keyring message is
tested with a vault that refuses every token (`TokenVault::Refusing`). The one test against the
real Secret Service is ignored by default:

```sh
cargo test -p review-store -- --ignored round_trips_through_the_secret_service
```

## Fonts

`crates/cv-base/src/platform/linux.rs` asks GNOME for the interface and monospace fonts
(`gsettings get org.gnome.desktop.interface font-name`, `monospace-font-name`), then fontconfig for
the files (`fc-match`), then looks for Cantarell, Ubuntu, Noto Sans or DejaVu Sans (UI) and DejaVu
Sans Mono, Noto Sans Mono or Ubuntu Mono (code) in `/usr/share/fonts`. A variable UI font
(Cantarell-VF) serves both weights. DejaVu Sans or Noto Sans Symbols 2 cover symbols; emoji come
from egui's bundled monochrome Noto Emoji (Noto Color Emoji holds bitmaps only, which egui does not
draw). With none of these installed, egui's bundled fonts close every family: Ubuntu Light for
text, Hack for arrows, bullets and shapes, Noto Emoji and emoji-icon-font for emoji. A test checks
that these alone draw every character the app's own strings use
(`theme::fonts::tests::bundled_fonts_alone_draw_symbols_and_emoji_in_every_family`); characters in
pull request content that no font has show as boxes.

## Renderer and garbled text

Code View draws with glow (OpenGL). Garbled text (fragments of other letters, missing letters) in
the top bar has one known cause in the app: the breadcrumbs keep laid-out text across frames and
noticed a new font atlas only by its fill ratio dropping, which a rebuilt atlas that refilled to the
same row does not show; they then painted glyphs at stale atlas positions. They now notice every
atlas rebuild (`crates/cv-widgets/src/atlas_watch.rs`). Whether that was the cause on the
machine that showed it is not verified; a GL driver that mishandles partial texture uploads
(common in virtual machines: VirtualBox, VMware, virgl) produces the same picture.

If text still looks wrong, find out whether the GL driver is the cause:

1. **Ctrl+Shift+F** shows the frame statistics with the scale and the font atlas size. Note both.
2. Try the other GL path: `MESA_LOADER_DRIVER_OVERRIDE=zink CodeView` (Mesa's GL on Vulkan), or
   software rendering: `LIBGL_ALWAYS_SOFTWARE=1 CodeView`. Clean text there means the hardware GL
   driver mishandles partial texture uploads.
3. Try wgpu (Vulkan) instead of glow. It is behind a cargo feature, so the default build stays small:

   ```sh
   cargo build --release -p review-app --bin CodeView --features wgpu
   CODEVIEW_RENDERER=wgpu target/release/CodeView
   ```

4. X11 versus Wayland: `WINIT_UNIX_BACKEND` is gone; start with `WAYLAND_DISPLAY= CodeView` to force
   X11 (XWayland), or unset `DISPLAY` to force Wayland.
5. Fractional scaling (125 %, 150 %): compare with 100 %. Report the scale with the screenshot.

## What is implemented

| Area | Linux | Where |
|---|---|---|
| Access tokens | Secret Service item per connection (see above); actionable message and "Save without the token" when none answers | `crates/review-store/src/platform/linux.rs`, `crates/cv-settings/src/onboarding/wizard/` |
| Data folders | `$XDG_CONFIG_HOME/ReviewApp` (settings, connections), `$XDG_CACHE_HOME/ReviewApp` (repositories, logs) | `crates/review-store/src/platform/linux.rs` |
| Browser extension | Host manifests `~/.config/google-chrome/NativeMessagingHosts/de.crasius.codeview.json` and `~/.config/microsoft-edge/NativeMessagingHosts/de.crasius.codeview.json`; the unpacked extension in `$XDG_CACHE_HOME/ReviewApp/browser/extension/`; a later start hands its link over the socket `$XDG_CACHE_HOME/ReviewApp/ipc/codeview.sock`, whose folder only the owner may enter (mode 0700). Details: [browser-extension.md](browser-extension.md) | `crates/cv-shell/src/browser/`, `crates/cv-shell/src/single_instance/platform/unix.rs`, `crates/review-store/src/paths.rs` |
| TLS | rustls + aws-lc-rs + platform verifier | `crates/review-core/Cargo.toml` |
| Fonts | GNOME/fontconfig fonts, common files, bundled fallbacks | `crates/cv-base/src/platform/linux.rs`, `theme/fonts.rs` |
| Reduced motion | GNOME "Reduce Animation" (`enable-animations`), read at start | `platform/linux.rs` |
| Downloads | `XDG_DOWNLOAD_DIR` from `~/.config/user-dirs.dirs`, else `~/Downloads` | `platform/linux.rs` |
| Show folder | `xdg-open` | `platform/linux.rs` |
| Window chrome | The system's decorations (X11 or Wayland server/client side); no integrated title bar | `platform/linux.rs` |
| Window id | Wayland app id and X11 class `de.crasius.codeview`; no `.desktop` file or packaged icon is installed yet, so docks show a generic icon | `platform/linux.rs` |
| Window placement, Save As dialog, refresh rate | Not implemented: centered default size, no "Download log" dialog (the button is hidden), frame limit without the monitor's rate | `platform/linux.rs`, `platform/portable.rs` |
| Git child processes | Nothing to hide | `crates/review-git/src/platform/other.rs` |
| Housekeeping priority | Normal priority | `crates/review-store/src/platform/linux.rs` |

## NOT-VERIFIED

Clippy and the workspace's tests pass in the container above; they never touch a desktop, a
keyring or a real host. None of this has run on a Linux desktop yet:

- Saving, reading and deleting tokens in GNOME Keyring, KWallet and KeePassXC; the unlock prompt;
  the message without a keyring; `secret-tool` showing the items.
- HTTPS through rustls against GitHub, GitLab and Azure DevOps, and a company CA from the system
  store.
- The fonts the desktop reports (Cantarell-VF's weight axis, Ubuntu's variable font), the symbol
  fallback, the reduced-motion setting, the XDG Downloads folder, `xdg-open`.
- The browser extension: registering the host manifests, Chrome or Edge starting the host, and a
  second start handing its link over the socket. Clippy for Linux has not run on this part either.
- Clean breadcrumbs on the machine that showed garbled text, on X11 and Wayland, at 100 % and at a
  fractional scale.
