---
paths:
  - "crates/review-store/**"
  - "crates/cv-base/src/platform/**"
  - "crates/cv-shell/src/single_instance/platform/**"
  - "crates/cv-shell/src/browser/platform/**"
  - "crates/review-git/src/platform/**"
  - "crates/review-app/src/main.rs"
---

# Desktop app: data folders, files, tokens, platform

## Data folders
- Every file the app keeps has its path in `review_store::AppPaths`: the config folder (settings, connections) and the cache folder (mirrors, logs, images, viewed state, threads, trash). Add a new file there; never build a data path elsewhere.
- Never write next to the executable, into the repo or into a machine-wide folder.
- Tests pass their own folders (`AppPaths::new`), so they never touch the owner's data. The platform folders per system are in `docs/linux.md`, `docs/macos.md` and `docs/windows.md`; name a new file there when the user may need to find or delete it.

## Files
- Write JSON through `atomic_file::write_json`: a temp file next to the target, then a rename. A crash never leaves half a file.
- A missing file means defaults. A corrupt file must not stop the app: start with defaults and tell the user when that loses real data.
- Caches have a limit and are cleaned by `review-store`'s retention (`retention/`), at a lower process priority where the system allows it.

## Tokens
- Tokens go only through `TokenVault`: DPAPI on Windows, the login Keychain on macOS, the Secret Service on Linux. The connections file holds a marker, not the token, except in a test vault.
- When the system store cannot keep a token, show the `VaultProblem` with its fix next to the action, never a bare error.
- Never log a token, a request header or a URL that carries one.

## Rights and processes
- The app runs as the user and never asks for admin rights.
- Child processes (`git`, the Mermaid renderer) start without a console window on Windows (`review-git/src/platform/`). A helper process that idles ends itself (the Mermaid renderer after `IDLE_EXIT`).
- The release build is a GUI program (`windows_subsystem = "windows"`): there is no console, so problems show in the UI or in the log files under the cache folder.
