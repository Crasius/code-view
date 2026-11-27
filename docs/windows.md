# Code View on Windows

Every release has builds for Windows x64 and Arm64, cross-built on Linux with `cargo xwin` (`docs/release.md`, Platforms). This page is about building and running Code View on a Windows machine. Windows is the system the project is developed on; the **NOT-VERIFIED** list at the end names what nobody checked on a fresh machine.

## What the build needs

| What | Why | Get it |
|---|---|---|
| Rust through rustup, MSVC host (`x86_64-pc-windows-msvc`) | The compiler; `rust-toolchain.toml` pins the version and rustup installs it on the first `cargo` command | <https://rustup.rs> |
| Visual Studio Build Tools with "Desktop development with C++" | The MSVC linker and the Windows SDK: the SDK's resource compiler embeds the icon and the version info (`crates/review-app/build.rs`, `winresource`) | Visual Studio Installer, or `winget install Microsoft.VisualStudio.2022.BuildTools` and pick the workload |
| Git for Windows | The app runs `git` for its mirrors and diffs, at build time for the tests and at run time; its Git Bash runs the repository's `sh` scripts | <https://git-scm.com/download/win> or `winget install Git.Git` |
| sccache, optional | Shares compiled dependencies between worktrees (`.agents/rules/worktrees.md`, Build cache) | `winget install Mozilla.sccache` |

TLS uses the system's own (SChannel through native-tls, `crates/review-core/Cargo.toml`), so no OpenSSL is needed. Tokens are kept with DPAPI, which needs nothing installed.

## Commands

From the repository root, in PowerShell or Git Bash:

```sh
cargo run -p review-app --bin CodeView                       # debug build, with a console for its output
cargo build --release -p review-app --bin CodeView           # target/release/CodeView.exe
cargo build --profile debugging -p review-app                # target/debugging/, for a debugger
```

Dev builds carry line tables only; a debugger session needs the full debug info of the `debugging` profile.

The checks before a pull request are `sh scripts/check.sh` and the tests of the changed crates (`AGENTS.md`, Commands). Scripts under `scripts/` and `.agents/hooks/` are POSIX sh: run them as `sh scripts/<name>.sh` (Git Bash's `sh`, also from PowerShell).

The release build is a GUI program without a console (`windows_subsystem = "windows"` in `crates/review-app/src/main.rs`); problems show in the app or in the log files under the cache folder. The debug build keeps its console.

A local release build links the C runtime dynamically, the Rust default for MSVC, so another machine needs the Visual C++ redistributable to run it [docs]. The shipped builds link it statically (`+crt-static`, `ci/build-release.sh`); to get the same locally, set `RUSTFLAGS=-C target-feature=+crt-static` for the build [untested].

## Running it

- **Data folders.** Settings and connections in `%APPDATA%\ReviewApp`, mirrors, logs and caches in `%LOCALAPPDATA%\ReviewApp` (`crates/review-store/src/platform/windows.rs`).
- **Tokens.** Encrypted with DPAPI for the current Windows user, inside the connections file; the file is useless on another account or machine (`crates/review-store/src/platform/windows.rs`).
- **Child processes.** `git` and the other tools start without a console window (`crates/review-git/src/platform/`, `crates/cv-base/src/platform/windows.rs`).
- **Optional tools.** `ffmpeg` and `ffprobe` beside `CodeView.exe` or on `PATH` play videos; `gh` and `az` let the setup take the token of a CLI sign-in; `claude` or `codex` drive the pull request assistant. A terminal opens in Windows Terminal (`wt.exe`) when it is installed, else in a console.
- **Browser extension.** Registering it writes registry keys under `HKCU` for Chrome and Edge (`docs/browser-extension.md`, Registration).
- **SmartScreen.** The builds are not code-signed, so SmartScreen warns about a downloaded `CodeView.exe`; More info, then Run anyway, starts it.

## NOT-VERIFIED

- A build on a fresh Windows machine with only the tools above; the exact Visual Studio components the Windows SDK needs.
- A native build on Windows on Arm (`aarch64-pc-windows-msvc`); the Arm64 release build is cross-built and has not run on such a machine (`docs/release.md`, Limits).
- A local release build with `+crt-static` running on a machine without the Visual C++ redistributable.
