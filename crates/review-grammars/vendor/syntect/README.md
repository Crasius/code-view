# syntect, vendored

- Upstream: https://github.com/trishume/syntect, crate `syntect` on crates.io.
- Base: 5.3.0 as published on crates.io (`src/`, `LICENSE.txt`, the published `Cargo.toml`), committed unchanged first so `git log -p` on this folder shows only the patches.
- Wired in: `[patch.crates-io]` in the root `Cargo.toml`. It is no workspace member; repo-lint and the stop gate skip it (rule `rust`, Dependencies).

## Why

syntect 5.3.0 pins fancy-regex 0.16, which builds a regex engine for every repeated fragment of a grammar's patterns and a full DFA for small ones. Measured on the eight languages of a .NET and web pull request (review-app, `memory_of_eight_languages_grammars_stays_small`): 162 MB compiled on one thread and 22 MB on each further thread with 0.16, 85 MB and 7.8 MB with the patches below. syntect's master has fancy-regex 0.20 but also a new grammar dump format that two-face's dumps do not load, so it cannot be a git dependency.

## Local patches

1. fancy-regex 0.20.0 instead of 0.16.2: upstream commit e471c92 ("Update fancy-regex to 0.20"), `Cargo.toml` and `Captures<str>` in `src/parsing/regex.rs`.
2. A 64 KiB lazy DFA cache per delegated regex engine instead of regex-automata's 2 MiB (`DELEGATE_DFA_CACHE_BYTES`, `src/parsing/regex.rs`): each thread that searches keeps one per regex.
3. `Cargo.toml` without the examples, tests, benches, dev-dependencies and profiles (their sources are not vendored), and `mismatched_lifetime_syntaxes` allowed: a path dependency builds with every lint on, and that lint is newer than 5.3.0.

## When to drop it

When a syntect release depends on fancy-regex 0.19.1 or later and two-face's grammar dumps load with it. Then depend on that release again; keep patch 2 only if the cache size still cannot be set from outside and the memory walkthrough's bound needs it.
