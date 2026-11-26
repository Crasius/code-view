# repo-lint

The repository rules that clippy cannot check. It reads the Rust files git knows (tracked, plus untracked files that are not ignored) outside `vendor/` folders, whose third-party code keeps its upstream style (rule `rust`, Dependencies), the workspace from `cargo metadata --no-deps` and the member manifests. Std plus `serde_json`, no network, about two seconds on the whole repository.

```sh
cargo run -q -p repo-lint                      # whole repository
cargo run -q -p repo-lint -- crates/cv-widgets   # only files and crates under a path
cargo run -q -p repo-lint -- --base main    # allowlist may not grow against this revision (default HEAD)
cargo run -q -p repo-lint -- --lengths         # shipped line counts over 400, longest first
cargo run -q -p repo-lint -- --shrink-allowlist  # lower allowlist entries to the current lengths
```

Exit 0 = clean, 1 = findings (one line each: `path[:line]: RLxxx name: message and fix`), 2 = the tool could not run. It runs in `sh .agents/hooks/repo-check.sh` (through `check-repo-lint.sh`) and in CI (`ci/lint.sh`, `.github/workflows/pull-request.yml`). The Rust rules it enforces live in `.agents/rules/rust.md`; this file documents the checks.

## Terms

- **Test code**: a file is test-only when every path to it from a Cargo target goes through a test or bench target or a test-only module declaration. A cfg is test-only when it holds only in test builds: `#[cfg(test)]`, `#[cfg(all(test, ...))]`, and the `test-support` feature that exports test helpers to other crates' tests (`#[cfg(any(test, feature = "test-support"))]`, also nested in `all(...)`); the shipped build never turns that feature on (RL015). In a shipped file, items under a test-only cfg are test lines.
- **Shipped lines**: a file's lines minus its test lines. Test-only files count zero.
- **Waiver**: a comment that starts with `repo-lint: expect(RL007) <reason>` waives the named rules on its own line (trailing comment) or the next line with code. Only RL003, RL004, RL007, RL009, RL012 and RL013 can be waived this way; the other rules have their own registry (allowlist, `.folder-budget`, the direction table). A waiver that matches no finding is RL014, so waivers can only shrink, like `#[expect]`.

## Rules

Ids are stable: a rule keeps its id, a retired id is never reused, and a retired rule's entry stays here naming its replacement.

### RL001 file-length
- Rule: a file over 700 shipped lines needs an entry in `file-lengths.allow`, and an allowlisted file may not grow past its entry, whatever its length. The rule text and the review part are RS-READ-04 in `.agents/rules/rust.md`.
- Why: a Rust file is the module boundary a reader scans. A long file hides its parts and usually holds more than one topic.
- Enforced by: repo-lint, counting shipped lines. `--lengths` lists the files over 400 lines for the review part.

### RL002 length-allowlist
- Rule: `file-lengths.allow` only shrinks. An entry above its file's length is lowered (`--shrink-allowlist`), an entry of a file within 700 lines or of a deleted file is removed, and an entry that is new or higher than at `--base` is refused.
- Why: the allowlist is debt that may only go down. A raised entry would silently undo RL001.
- Enforced by: repo-lint, comparing with `git show <base>:tools/repo-lint/file-lengths.allow`. A file that git reports as renamed since the base (`git diff -M --name-status <base>` over the listed paths) keeps the entry of its old path: a move with `git mv` is not a new entry, a raise on the way still is. The default base `HEAD` catches only a raise that is not committed yet; a branch runs with `--base <the branch it forked from>` (rule `lanes`), CI with the pull request's base or the commit before the push.

### RL003 role-suffix
- Rule: no `struct`, `enum`, `trait` or `type` whose name ends in `Manager`, `Service`, `Helper` or `Handler` (RS-DONT-08).
- Why: a role suffix names no responsibility, so the type grows without a boundary. Name the work: `DiffLoader`, `TokenVault`.
- Enforced by: repo-lint on declarations. Clippy's `disallowed_names` covers only bindings.

### RL004 vague-module
- Rule: no module named `utils`, `util`, `helpers`, `helper`, `common` or `misc` (RS-DONT-08). The `cv-services` crate is a named topic (the app's connections, writes, checkouts and agents) and is not on the list.
- Why: such a module collects unrelated items; each belongs with its topic.
- Enforced by: repo-lint on `mod` declarations and inline modules.

### RL005 crate-direction
- Rule: workspace crates depend only on the crates the direction table in `src/manifest.rs` allows: `review-core` on none, `review-git`, `review-store`, `review-agent` and the providers on `review-core` only, `repo-lint` on none. Each `cv-*` UI crate has a layer and its domain crates; it may also use every `cv-*` crate of a strictly lower layer. `review-app` (the bin, top layer) may use all of them. The table is the layer table of `.agents/rules/rust.md` (Workspace). A new crate needs a row (RS-DO-03).
- Why: the direction keeps domain crates free of UI and of each other, so they test without a GPU and a provider change cannot reach another provider.
- Enforced by: repo-lint on `cargo metadata` (normal, optional, dev and build dependencies).

### RL006 ui-dependency
- Rule: only `review-app` and the `cv-*` crates depend on `egui`, `eframe`, `egui_extras`, `egui_kittest` or `resvg` (RS-DO-03).
- Why: UI crates in a domain crate link rendering code into it and make its tests need a window.
- Enforced by: repo-lint on `cargo metadata`.

### RL007 unsafe-outside-platform
- Rule: the `unsafe` keyword appears only in files under a `platform/` folder (or a `platform.rs` file) (RS-DONT-03).
- Why: FFI stays in small, reviewed, OS-specific modules behind safe functions; UI and domain code never needs unsafe.
- Enforced by: repo-lint on code tokens (strings and comments excluded). Inside platform modules, clippy's `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` and the `unsafe_code` lint with `#![expect(unsafe_code, reason)]` apply.

### RL008 folder-budget
- Rule: a module folder under `src/` holds at most 12 shipped `.rs` files (`mod.rs`, `lib.rs`, `main.rs` and test-only files not counted). Above that, split it into sub-modules or add a `.folder-budget` file whose first line says why the files belong together. A budget file in a folder within the limit is stale (RS-READ-09).
- Why: a flat folder of many files hides its sub-topics the way a long file hides its parts.
- Enforced by: repo-lint, adapted from platform-dotnet ELY0010/ELY0011 with a looser threshold.

### RL009 deny-warnings
- Rule: no `#![deny(warnings)]` or `forbid(warnings)` in source (RS-DONT-07).
- Why: it turns every new compiler or clippy release into a build break for anyone building the code. `-D warnings` belongs on the command line (CI, ship).
- Enforced by: repo-lint on attributes.

### RL010 member-profile
- Rule: `[profile.*]` tables live only in the root `Cargo.toml` (RS-PERF-05).
- Why: Cargo ignores profiles in member manifests, so a setting there silently does nothing.
- Enforced by: repo-lint on member manifests.

### RL011 workspace-dependency
- Rule: every dependency of a member is declared once in `[workspace.dependencies]` and used with `workspace = true`; a member may add features (rule `rust`, Dependencies).
- Why: one place for versions and feature choices keeps the lock file free of duplicate versions and makes every dependency visible in one table.
- Enforced by: repo-lint on member manifests.

### RL012 todo-comment
- Rule: no comment that opens with `TODO` or `FIXME`, or carries `TODO:`/`TODO(` (AGENTS.md, Done).
- Why: an open note in a commit is unfinished work that nobody tracks.
- Enforced by: repo-lint on comments. Clippy's `todo` and `unimplemented` cover the macros.

### RL013 block-on
- Rule: no `block_on` in shipped code under `crates/review-app/src/` or `crates/cv-*/src/`, except `cv-testkit`, which only tests link (RS-DONT-06). Test drivers may use it.
- Why: the update path never blocks, and any other app thread that blocks on async work stalls the one the UI waits on. Clippy's `disallowed-methods` cannot list `Runtime::block_on`, because `#[tokio::test]` and `#[tokio::main]` expand to it with the caller's span.
- Enforced by: repo-lint on code tokens outside test code.

### RL014 expectation
- Rule: a waiver comment names a waivable rule, gives a reason of at least ten characters, and matches a finding.
- Why: waivers are the suppression registry; a stale or unexplained one hides nothing and misleads.
- Enforced by: repo-lint.

### RL015 test-support-feature
- Rule: a member manifest turns on a dependency's `test-support` feature only under `[dev-dependencies]` (any target), or forwards it from its own `[features]` list (`test-support = ["cv-base/test-support"]`). Never in a normal or build dependency (rule `testing`).
- Why: the feature exports fakes and probes to the tests of the crates above. In a normal dependency it would link them into `CodeView`; the release build's const assert on `cv_base::TEST_SUPPORT` is the second guard.
- Enforced by: repo-lint on member manifests.

### RL016 motion-install
- Rule: `motion::install` appears only in the shipped code of `crates/review-app/src/main.rs`, where the binary creates the app. `ReviewApp::new` does not call it, so the whole-app tests that build the app never read the setting. Test code pins motion with `motion::set_reduced(ctx, false)` (or `true` for a reduced-motion case), or leaves it unset, which animates.
- Why: `motion::install` reads the machine's reduced-motion setting. A test that calls it runs differently on a machine with Reduce Motion on: GitHub's macOS runners have it on, and a fling test and four whole-app walkthroughs failed only there.
- Enforced by: repo-lint on code tokens (the path `motion::install`, strings and comments excluded); the test `app::motion_check` proves that a harness-built app animates with the setting unset.

## Adding a rule

Add a `Rule` variant with the next free id in `src/finding.rs`, a check with focused tests for its parsing or decision logic, and an entry here (Rule, Why, Enforced by). Land it together with the fixes or registry entries for its existing findings, so the repository stays clean.
