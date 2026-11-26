# Code View

Code View is a native desktop app for reviewing pull requests on GitHub, GitLab and Azure DevOps. It is written in Rust with [egui](https://github.com/emilk/egui), starts fast and stays small, and puts the code diff first: the changed files, their diffs and the comment threads on them, without a browser.

It is for developers and reviewers who read many pull requests across several hosts and want one fast window for all of them.

- **Hosts:** GitHub and GitHub Enterprise Server, GitLab (gitlab.com and self-managed), Azure DevOps Services and Server.
- **Systems:** Windows 10 or newer (x64, Arm64), macOS 12 or newer (Apple silicon and Intel), Linux with glibc 2.31 or newer (x64, arm64). The table in [docs/release.md](docs/release.md#platforms) says how each build is made.

## Download

Releases are on Creative Downloads: <https://creative-downloads.com/s/SSknYmWQ>. The product page is <https://codeview.northbyte.sh>.

The builds are not code-signed or notarized, and no checksums are published (docs/release.md, Limits). Windows SmartScreen and macOS Gatekeeper warn about a downloaded build; [docs/macos.md](docs/macos.md) shows how to open the macOS app anyway.

## License

Code View is source-available, not open source: it is licensed under the [Functional Source License 1.1, Apache 2.0 future license](LICENSE) (`FSL-1.1-ALv2`). You may use, change and share it for any purpose, including commercial use at work, except a Competing Use: a product or service that competes with Code View. Each released version becomes available under Apache 2.0 on the second anniversary of the day it was made available.

## Source

The public repository, which takes issues and pull requests, is <https://github.com/Crasius/code-view>.

## Build from source

You need git and Rust through [rustup](https://rustup.rs); `rust-toolchain.toml` picks the exact toolchain. What else each system needs, and the commands, are on its own page:

- [docs/windows.md](docs/windows.md)
- [docs/macos.md](docs/macos.md)
- [docs/linux.md](docs/linux.md)

The short form on every system: `cargo run -p review-app --bin CodeView`.

## The crates

The workspace is one binary and its libraries, in layers: a crate uses only domain crates and `cv-*` crates of a lower layer. The layer table with every crate is in [.agents/rules/rust.md](.agents/rules/rust.md) (Workspace); `tools/repo-lint` enforces it.

| Crates | What they hold |
|---|---|
| `review-core` | The pull request model, the `Provider` trait, the HTTP client |
| `provider-github`, `provider-gitlab`, `provider-azure` | One host each: its API calls and their mapping to the model |
| `review-git`, `review-store`, `review-agent`, `review-grammars` | Git mirrors and diffs, the data folders and the token vault, the assistant's agent CLIs, the syntax grammars |
| `cv-base` to `cv-pr-page` | The UI, from the platform layer and widgets up to the pull request page |
| `review-app` | The `CodeView` binary: the window and the title bar |
| `tools/repo-lint` | The repository rules clippy cannot check |

The browser extension that opens pull request links in the app is in `browser-extension/` ([docs/browser-extension.md](docs/browser-extension.md)).

## What Code View does on your machine

**Network.**

- It talks to the hosts of the connections you add: their REST and GraphQL APIs (for example `api.github.com`, `crates/provider-github/src/client.rs`), for Azure DevOps also `vssps.dev.azure.com` for identities (`crates/provider-azure/src/client.rs`), and git over HTTPS for the repository mirrors it keeps (`crates/review-git/src/mirror.rs`).
- Images and videos that pull request text embeds are downloaded from where they are hosted. Credentials go only to the connection's own hosts; other hosts are asked anonymously (`crates/cv-media/src/images.rs`).
- Links such as the documentation, the help post or the support page open in your browser; the app does not fetch them (`crates/cv-shell/src/nav_rail.rs`, `crates/review-store/src/token_vault.rs`, `crates/cv-pr-assistant/src/view.rs`).
- There is no telemetry, no crash reporting and no self-update.

**Processes it starts.**

- `git`, for mirrors, diffs and checkouts (`crates/review-git/src/command.rs`).
- Itself: as the Mermaid diagram renderer (`--mermaid-worker`, `crates/cv-media/src/mermaid/worker.rs`), and, when the browser extension asks, as its native messaging host, which may start the app with a link (`crates/cv-shell/src/browser/host.rs`).
- `ffmpeg` and `ffprobe` to play videos, when they are beside the app or on `PATH` (`crates/cv-media/src/video/mod.rs`).
- `gh auth token` or `az account get-access-token`, only when you choose "Use my GitHub CLI / Azure CLI sign-in" in the setup (`crates/cv-settings/src/onboarding/cli.rs`).
- The `claude` or `codex` CLI for the pull request assistant, when you use it, with the hosts' access tokens removed from its environment (`crates/review-agent/src/process.rs`).
- Platform helpers when you ask for them: the file manager (`explorer`, `open`, `xdg-open`), a terminal, and on macOS and Linux the system's dialogs and font settings (`osascript`, `zenity` or `kdialog`, `gsettings`, `fc-match`) (`crates/cv-base/src/platform/`).

**What it writes.**

- Its settings and connections in the config folder and its mirrors, logs and caches in the cache folder (Windows: `%APPDATA%\ReviewApp` and `%LOCALAPPDATA%\ReviewApp`; macOS and Linux: their pages in `docs/`) (`crates/review-store/src/paths.rs`, `crates/review-store/src/platform/`).
- Access tokens only through the system's store: encrypted with DPAPI for your Windows user, in the login Keychain on macOS, in the Secret Service on Linux (`crates/review-store/src/token_vault.rs`). They are never written in plain text or logged.
- Only when you click **Register** for the browser extension (Settings, Links): the native messaging manifest `de.crasius.codeview.json` and, on Windows, the registry keys `HKCU\Software\Google\Chrome\NativeMessagingHosts\de.crasius.codeview` and `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\de.crasius.codeview` that name it; **Unregister** removes them (`crates/cv-shell/src/browser/registration.rs`, `crates/cv-shell/src/browser/platform/windows.rs`, [docs/browser-extension.md](docs/browser-extension.md#registration)).
- No autostart entry, no URL protocol handler, no service, and it never asks for administrator rights.

## Contributing and security

- How to contribute: [CONTRIBUTING.md](CONTRIBUTING.md). Contributions need the contributor license agreement in [CLA.md](CLA.md).
- How to report a security problem: [SECURITY.md](SECURITY.md). Please do not open a public issue for it.
