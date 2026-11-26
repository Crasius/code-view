# Release builds and CI

The source of truth is the project on a self-managed GitLab instance, the remote `origin`; GitHub carries its public mirror, the remote `github` (rule `git-flow`). The GitLab pipeline (`.gitlab-ci.yml`, `ci/checks.yml`, scripts in `ci/`) checks every merge request and every push to `main` (Pipelines). A release tag builds the app for every platform and publishes those builds to Creative Downloads, in one release with a depot per platform. `.github/workflows/pull-request.yml` is separate and the only workflow on GitHub: it checks pull requests (GitHub), a daily run keeps its caches warm, and a maintainer's manual run also packages the app (`docs/macos.md`). No push starts it.

This page is the maintainer's runbook. Building on your own machine is on the platform pages: `docs/windows.md`, `docs/macos.md`, `docs/linux.md`.

## Cut a release

1. In a worktree, set `version` in `[workspace.package]` of the root `Cargo.toml`, run `cargo check` so that `Cargo.lock` follows, merge into `main` and push `main`.
2. When that commit's pipeline is green, tag it and push the tag: `git tag vX.Y.Z <commit>`, `git push origin vX.Y.Z`.
3. The tag's pipeline runs the checks and builds again, then job `creative-downloads` publishes. Its artifacts `publish.json` and `release-notes.md` show what went out.

The tag must equal the version in `Cargo.toml`: every build job stops otherwise (`ci/build-release.sh`). Only a protected tag publishes; `v*` tags are protected, so only Maintainers can create one. The release notes are the commit subjects since the previous tag (`ci/release-notes.sh`).

## Pipelines

The instance has one shared runner, so a pipeline runs only what verifies something not yet verified.

| Pipeline | Jobs |
|---|---|
| Merge request that changes a cargo input | `lint`, `test` (the tests the change needs), `secret-scan` side by side; `build-linux-x64` after them when a release input changed; `measurements` and every other build wait for a manual start |
| Merge request that changes no cargo input (docs, `.agents/rules`, `AGENTS.md`) | `secret-scan` |
| Push to `main` | `main-scope` and its child pipeline `main-checks`: one short job when the push is a merge GitLab made from a merge request whose head has the same tree, else `lint`, `test` (every test), `measurements` and `secret-scan`, which write the build caches |
| Protected tag `vX.Y.Z` | `lint`, `test` (every test), `secret-scan`, the five builds, `creative-downloads`; every cache only restored |
| Run by hand (web, API) | `lint`, `test` (every test), `secret-scan`, the five builds; on `main` it writes the build caches |

- `lint` runs format, `repo-check.sh`, repo-lint, both clippy passes and the clippy pass of `review-app`'s feature `measurements`; `test` runs the tests (Tests). Each has its own build cache (Build cache).
- `measurements` runs `review-app`'s heap measurements, which compile only with the feature `measurements` (rule `testing`, Running), from `test`'s cache, which it reads and never writes: on a warm cache only `review-app` compiles again, so the job should end before `test` [untested until main's first child pipeline]. GitHub does not run it.
- Every cargo job has a timeout of 1 h (`.rust` in `ci/checks.yml`); the one `check` job before them had 2 h. The slowest, a cold `test`, took 13 minutes (below).
- The cargo inputs are `.rust-inputs` in `ci/checks.yml`. `.gitlab-ci.yml` and `ci/` are on the list, so a CI change runs the jobs it defines.
- The release inputs are `.release-inputs` in `.gitlab-ci.yml`: the manifests, the lockfile, the build scripts, the toolchain and the release scripts. Only they can break a release build that `lint` and `test` passed, chiefly through a dependency whose license `ci/about.toml` does not accept.
- `ci/main-scope.sh` relies on the project setting "Pipelines must succeed": GitLab then merges a request only after the pipeline of its head passed. A local merge pushed to `main` has no such pipeline and runs the checks.
- A new push to a merge request cancels its running pipeline (`auto_cancel`), all jobs but `creative-downloads` are interruptible.
- `main` builds nothing: the tag's pipeline builds and publishes its own.

Measured on 2026-11-26 in merge request !7 [observed]. Before, one job `check` ran everything in 802 s (pipeline 4896). Now `lint` and `test` run side by side, so a pipeline takes as long as `test`:

| Run | `lint` | `test` | Pipeline |
|---|---|---|---|
| Cold, no build cache (4905) | 314 s (4900) | 761 s: build 474, tests 145, cache upload 121 | 761 s |
| One crate changed, warm (4906) | 83 s | 382 s: restore 74, build 8, tests 150, upload 137 | 381 s |
| No crate changed, warm (4907) | 41 s | 236 s: restore 76, build 2, tests 145, no upload | 236 s |
| Docs only (4607) | skipped | skipped | 12 s, `secret-scan` alone |
| Every test, a merge request restoring caches without build ids (4909, job 25100) | 282 s, cold: the key now hashes `clippy.toml` | 230 s: restore 55, build 62 (every test executable linked again, Build cache), tests 97, no upload | 230 s; `measurements` 202 s beside it |

Merge request pipelines upload no cache since 4909 ("Not uploading cache … due to policy"); the one-crate row's upload of 137 s is gone from them.

The floor of `test` is the cache restore and `review-app`'s tests (about 110 s in one process); a change to a crate below `review-app` adds the rebuild of `review-app` (about 65 s cold).

## Platforms

The instance's only runner is Linux amd64, so everything but Linux x64 is a cross-build. `ci/build-release.sh` builds one platform into `dist/<platform>/`; `ci/publish-creative-downloads.sh` publishes those folders.

| Depot | Target | Tool (image) | Runs on |
|---|---|---|---|
| `windows:x64` | `x86_64-pc-windows-msvc` | `cargo xwin` (`cargo-xwin`) | Windows 10 or newer |
| `windows:arm64` | `aarch64-pc-windows-msvc` | `cargo xwin` (`cargo-xwin`) | Windows 10 or newer on Arm |
| `linux:x64` | `x86_64-unknown-linux-gnu.2.31` | `cargo zigbuild` (`cargo-zigbuild`) | glibc 2.31 or newer, with the libraries of `docs/linux.md` |
| `linux:arm64` | `aarch64-unknown-linux-gnu.2.31` | `cargo zigbuild` (`cargo-zigbuild`) | glibc 2.31 or newer, with the libraries of `docs/linux.md` |
| `mac` | `universal2-apple-darwin` | `cargo zigbuild` (`cargo-zigbuild`) | macOS 12 or newer, Apple silicon and Intel |

All images are pinned by digest in `.gitlab-ci.yml`. `ci/install-build-deps.sh` unpacks `cargo-about` on top of a build job's image.

Every system needs `git` at run time; the app runs it for its mirrors and diffs.

### Windows
- `+crt-static`: a fresh Windows need not have the Visual C++ redistributable.
- `/IGNORE:4099`: the CRT libraries name debug files that only Microsoft has, and the release profile strips debug info anyway. `lld-link` warns about each one otherwise.
- The icon and the version info come from `crates/review-app/build.rs` through `winresource`, which calls `llvm-rc` on a Linux host. That is why the build dependency has no Windows-only cfg: a build dependency's cfg reads the host.

### Linux
- `.2.31` in the target makes zig link against glibc 2.31, the version of Debian 11 and Ubuntu 20.04, so one binary runs on every newer distribution. The programs need only libc, libm, libdl, libpthread and the loader at link time [observed 2026-11-05]; X11, Wayland and GL load at run time (`docs/linux.md`).
- zig's C compiler builds `aws-lc-sys`, the crypto of rustls on Linux. Nothing else in the build needs C or an assembler.

### macOS
- Creative Downloads takes one macOS depot without an architecture, so one universal binary serves both architectures.
- The depot holds `Code View.app`; `ci/build-release.sh` writes its `Info.plist` with the values of `[package.metadata.bundle]` in `crates/review-app/Cargo.toml`. `scripts/package-macos.sh` stays the way to build on a Mac, with a disk image and a full ad-hoc signature of the bundle.
- The minimum system is `MACOSX_DEPLOYMENT_TARGET`, the SDK comes with the image.
- The arm64 slice carries the linker's ad-hoc signature, without which Apple silicon refuses to run a binary. The x86_64 slice is unsigned [observed in the load commands, 2026-11-05]. The login Keychain ties saved tokens to a signature, so a new build may ask once per token (`docs/macos.md`).

## Runner

The jobs run on the GitLab instance's shared runner, as pods of the Kubernetes executor. The instance's administrators set it up; its limits, from their runner documentation, shape the jobs [docs]:

- Every capability is dropped: a job's root cannot switch users, change owners or write to another user's files. So `ci/install-build-deps.sh` unpacks a tool into `.build-deps` instead of installing it. `docker run --cap-drop=ALL --security-opt no-new-privileges` behaves the same way on a local machine.
- A job has 4 CPUs and 8 GiB of memory.
- A cache archive passes through the helper's `/cache` of 2 GiB, and the runner uploads none above 1 GiB: a larger one stays on the pod and is lost [observed 2026-11-26, pipeline 4900: "Cache archive size (1158212488) is too big (Limit is set to 1073741824)"]. The release builds cache only what cargo and xwin downloaded; `lint` and `test` each cache their `target`, `test` in two archives (Build cache).
- A build container may use 8 GiB of disk and the helper 4 GiB, and the job's folder counts towards them. So jobs build without incremental data and without debug info, and each prints what its folders take at its end.
- Jobs of one pipeline run in pods of their own, side by side [observed 2026-11-26: `lint` and `test` of pipeline 4900 started together].

`lint` and `test` run in the official Rust image, the one `docs/linux.md` names: its git and its DejaVu fonts are all the tests need.

## Build cache

`lint` and `test` share no compiled unit (clippy writes check metadata, the tests need code), so each job kind has a cache of its own `target`, keyed by `Cargo.lock` and `rust-toolchain.toml` (`lint`: `Cargo.lock` and the root's `*.toml`, so that a change to `clippy.toml` starts it cold); the downloads are a third cache both share.

- **Only `main` writes.** Every cache's policy is `CACHE_POLICY`: `pull` by default (`ci/checks.yml`), `pull-push` in the pipelines of `main`, a push's child pipeline (`ci/main-scope.sh`) and a run by hand (`.gitlab-ci.yml`, workflow). Nearly every change reaches `main` as a pushed local merge, so main's child pipeline is the ordinary pipeline and keeps the caches warm. A merge request and a tag only restore what `main` wrote, so a tag's pipeline builds from caches no merge request could have written.
- **Why that is needed:** the project separates the caches of protected and unprotected refs, yet a merge request pipeline the owner starts gets the `-protected` keys too [observed 2026-11-26, job 25060 of !7: "Creating cache target-test-…-protected"]. GitLab's documentation says why [docs, CI/CD caching, since GitLab 18.4.5]: "Cache keys receive the `-protected` suffix if the pipeline: Runs for a protected branch or tag. [...] Was started by a user with the Maintainer or Owner role." The role of whoever starts a pipeline decides, not only its ref; a Developer's merge request pipeline gets the `non_protected` keys, finds no cache of `main` and builds cold.
- Each job writes its cache back only when the build took longer than the upload would (`ci/lint.sh`, `ci/test.sh`): uploading `test`'s two archives takes about 2 minutes, and a job that rebuilt one crate in 8 s would only make the next job's rebuild 8 s shorter. So the cache drifts behind `main` until a build costs more than an upload, and then catches up.
- A fresh clone dates every file to the clone's time, so cargo would take every workspace crate for changed. `ci/target-stamp.sh` fixes that by content: `save` records the tree the cache was built from (`git ls-files -s`), `restore` dates every tracked file and folder back to 2001 and touches only the paths whose blob differs. cargo then rebuilds the crates whose inputs changed and what depends on them, whichever commit of `main` wrote the cache: content decides, not the commit's age.
- `test`'s cache is two archives (the runner's upload limit), uploaded one after the other. A job cancelled between the two uploads, or two pushes to `main` whose jobs write one key at once, would leave the record and fingerprints of one build beside the test executables of another, and cargo would run old tests as fresh. So `save` writes one build id into both archives, and `restore` deletes every test executable when the two differ; cargo then links them again and the rest of the cache stays warm.
- Registry crates are fresh by version, not by time; a lockfile or toolchain change starts a new key, and that job kind builds cold once. A merge request that changes `Cargo.lock` builds cold, since `main` has no cache for its key yet. Falling back to the cache of the previous lockfile was rejected: GitLab's fallback keys are fixed names, not "the newest key with this prefix", and a `target` kept across dependency sets collects the artifacts of every old version until its archive passes the upload limit and the runner drops it.
- Every job that shares a cache uses the same `CARGO_*` profile settings (`.rust` in `ci/checks.yml`), or the units' hashes differ.
- To start cold, clear the runner caches on the project's CI/CD pipelines page.

## Tests

`test` builds every test of the workspace and runs, with cargo-nextest (pinned in `ci/install-build-deps.sh`, profile `ci` in `.config/nextest.toml`), those the change needs: `scripts/affected-tests.sh` maps the changed files to the packages that own them or whose sources name them, and selects those packages and everything that depends on them through any dependency, dev-dependencies included. A change to a shared input (the lockfile, the root manifest, the toolchain, `.cargo/`, `.config/`, `.gitattributes`, the CI files, `cv-testkit`, a build script) selects every test; docs and agent rules select none.

- The build stays `--workspace`: `cargo test -p` resolves features for the selected packages only and rebuilt 124 dependencies for `-p review-app` [observed 2026-11-26, local], which a warm cache would lose. Only the run is filtered.
- Only a merge request runs a selection, compared with its diff base. main's child pipeline, a tag and a run by hand run every test: a selection against the commit before a push would leave untested what an earlier push changed whose pipeline was cancelled, failed or skipped, and on a warm cache every test took 232 s, about as long as a selection (pipeline 4908) [observed 2026-11-26].
- nextest runs each test in a process of its own. It is no faster here (`review-app`'s walkthroughs pay their start-up per test), but it filters the run without changing the build. So nextest runs every test binary but `review-app`'s: that one runs as `cargo test` runs it, all its tests in one process with one thread per CPU, when the selection holds any of them (`ci/test.sh`), because under nextest four walkthroughs at once timed out twice in three runs. nextest runs no doctests; `cargo test --doc` follows it. Each of the three groups runs also when another failed, and the job fails at the end.

## Secret scan

Job `secret-scan` reads the full history with gitleaks. `.gitleaksignore` names the four findings that are no secrets, each by commit, file, rule and line, so a new finding still fails the job.

## Public mirror

The public repository is <https://github.com/Crasius/code-view>. It is written only by the GitLab project's push mirror; nobody pushes or merges there (rule `git-flow`).

- **GitLab side:** Settings, Repository, Mirroring repositories: one push mirror to `ssh://git@github.com/Crasius/code-view.git`, SSH public key authentication, protected branches only. So `main` is mirrored and no other branch is; the page shows the last run and its error.
- **GitHub side:** the mirror's public key is a deploy key with write access (Settings, Deploy keys). The ruleset "main comes only from the GitLab mirror" restricts updates, deletions and force pushes of `main` and lets only deploy keys bypass it. Merging is limited to merge commits, GitHub Actions need approval for every outside contributor's pull request, and private vulnerability reporting and secret scanning are on.
- **A mirror run that fails:** read the error on the GitLab page first. A rejected key means the deploy key was removed or lost its write access: read the key again (`GET /projects/:id/remote_mirrors/:mirror_id/public_key`) and add it as a deploy key.
- **The history before the public root** (2026-11-26) is in the private GitLab project `northbyte/code-view/app-archive`, with the release tags up to `v0.1.2`. Pipelines are off there. Nothing from it may reach this project or GitHub (rule `git-flow`, A branch from before the public root).

## Third-party notices

Every depot holds `THIRD-PARTY-NOTICES.txt` next to the program (macOS: in the bundle's `Contents/Resources`), as the licenses of the shipped crates ask. `ci/build-release.sh` writes it with `cargo-about`, pinned by version and checksum in `ci/install-build-deps.sh`; the config and the template are `ci/about.toml` and `ci/about.hbs`.

- It lists only what links on that target, and a crate under a license outside `accepted` stops the build. MPL-2.0 asks to say where the source is: each entry names its repository.
- The build appends the license of the Phosphor icon fonts and those of the syntax grammars. `crates/review-grammars/build.rs` bakes bat's grammars into the program through a build dependency, which the crate list leaves out, so `crates/review-grammars/examples/grammar_notices.rs` prints them.
- The C# grammar comes from Sublime Text's packages under MIT; its license text is `crates/review-grammars/grammars/csharp-license.txt`, which the build appends too. The PowerShell grammar beside it is the project's own and falls under `LICENSE`.
- The Rust standard library and Zig's compiler runtime are named by hand in `ci/about.hbs`, since no crate brings them. egui's fonts come through `epaint_default_fonts`.
- The app's own crates stay out of the list: `ci/about.toml` skips workspace crates that are not published (`publish = false`).

## CI variables

All three are protected, so only `main` and protected tags can read them. Set them in the project's CI/CD settings or with `glab variable set`.

| Variable | Holds |
|---|---|
| `CDL_TOKEN` | Creative Downloads token with the scopes `creations:read` and `packages:publish`. Also masked. |
| `CDL_CREATION` | The creation "Code View" |
| `CDL_VARIANT` | Its variant |

Creative Downloads' CI template (`northbyte/creative-downloads/cli`, `templates/cdl.gitlab-ci.yml`) installs `cdl` in the job. That project's job token allowlist must name this project, or the install fails.

## Renew the token

The token expires 180 days after it was made, the current one on 2027-05-04; job `creative-downloads` cannot publish after that. After the owner has logged in with `cdl auth login` and `glab auth login` (to the GitLab instance), pipe the new secret straight into the variable, so that it reaches neither a file nor a log. Run it in the main checkout: `glab` takes the instance and the project from the remote `origin` [docs: glab reads the repository from its git remotes].

```sh
cdl token create --name "code-view release job" --expires-in 180 \
    --scope creations:read --scope packages:publish \
  | glab variable set CDL_TOKEN --masked --protected
```

## Community

The community "Code View" (`code-view`) on Creative Downloads shows the creation, an about page, a gallery and the posts that introduce the app. Its posts and page live there, not in this repository: `cdl post show <id> --body` prints a post's Markdown, `cdl post update <id> --file <file>` replaces it. Markdown tables stay plain text there, so the posts use lists.

The app links to the public post "Getting started with Code View" for the setup of the Linux secret store (`LINUX_HELP_URL` in `review-store`). Keep that post's title: its address comes from the title.

The owner asked for the screenshots in the posts, which is the one exception to rule `ui`. They come from the Linux release build on a virtual screen in a container, with anonymous connections to public repositories, so no window reaches the desktop, no token is in reach and no private pull request shows. What they show is public data of those projects.

The pages crop the images with `object-fit: cover`, centred, so each image is drawn for its crop [observed 2026-11-05]. Check a new image on the live pages before calling it done.

- **Creation header:** the background of the whole download page, scaled to 1920x1080 by the server. A card of 900x456 covers its middle (358x389 on a phone), and a wide window cuts its top and bottom. So it holds no text and no logo.
- **Creation cover:** 260x390 in that card (96x144 on a phone), and a centred square of 40 to 64 px in the community's download lists. So the icon and the name sit inside the middle square of the image.
- **Community cover:** 6:1 on the community page (1072x179), 3.5:1 on a phone (390x111), and 291x160 on the publisher page. So all text and the logo sit in the middle 60 % of the width and the middle half of the height.
- **Community thumbnail:** a circle of 64 to 72 px, with the icon alone.
- **Creation thumbnail:** no public page showed it.

## Limits

- The builds are neither signed nor notarized. Windows SmartScreen and macOS Gatekeeper warn on a file that came through a browser.
- Nobody reviewed the legal side of the public release: the name, the terms of the Microsoft C runtime that `cargo xwin` links statically, the license (`LICENSE`) and the contributor license agreement (`CLA.md`).
- No checksums are published next to the builds.
- CI runs the tests on Linux only. Windows and macOS code is compiled in the release builds, not tested.
- Nothing starts the macOS or the Windows arm64 build before it ships; no runner has those systems.
- The site shows the server's package number and the size of the largest build. The tag appears only in the release notes, and `cdl publish` has no option to set that number [observed 2026-11-05 on the YT Music release, `cdl` 0.3.0].
- A release's download page lists every build under the name `CodeView`, with the system on a second line [observed 2026-11-05 on the YT Music release, which names its depots the same way]. The cause is on the platform's side, so the depot names stay as they are until the platform labels builds by their system.
- A Maintainer's pipeline on an unprotected branch or a merge request gets the `-protected` cache keys of `main` and the tags (Build cache) [docs]. The rules keep every pipeline but main's at `policy: pull`, but a Maintainer can change the rules on a branch and write those caches. Look at this again before adding a Maintainer to the project.
