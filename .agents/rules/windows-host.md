# Windows host

This project runs on Windows with two shells: PowerShell and a POSIX shell (Git Bash). Each rule names its reason.

## Shells
- **One dialect per command.** PowerShell and POSIX sh differ: `;` against `&&`, `2>$null` against `2>/dev/null`, here-strings against heredocs, `$env:X` against `$X`. Choose the shell first, then write the whole command in its dialect. Why: a mixed command fails to parse or does something else.
- **Resolve bash before you use it.** In PowerShell, `bash` can resolve to WSL (`System32`) or a Store alias (`WindowsApps`). Check `Get-Command bash`. When it resolves there, call Git Bash by its full path. Why: WSL cannot open the Windows checkout paths, and its missing tools look like missing project tools.

## Files and paths
- **File tools over shell reads.** Read, search and edit files with the harness file tools, not with `cat`, `head`, `sed -n`, `grep`, `find` or `sed -i`. Why: each shell call starts a new shell process, which is slow on Windows, and a recursive shell search walks ignored folders such as `node_modules`. Use the shell for builds, tests, git and real pipelines.
- **Absolute paths.** Agent shells reset the working directory between calls. A `cd` in one call binds neither the next call nor the file tools. Give every path in full.
- **Stop path rewriting.** Git Bash turns an argument that starts with `/` into a Windows path: `/api/items` becomes `C:/Program Files/Git/api/items`. Prefix native Windows tools and slash-leading arguments with `MSYS_NO_PATHCONV=1`, or send the payload from a file. PowerShell does not rewrite paths.
- **No `:` in file names.** On NTFS, `name: title.md` creates an empty file with a hidden data stream. Map `:` to `_` when you build a file name from a title.
- **Junctions when symlinks need admin rights.** Creating a symlink on Windows needs admin rights or developer mode. Link folders with a junction instead: `New-Item -ItemType Junction -Path <link> -Target <folder>`. A junction links local folders only.

## Command-line length
- Windows caps one command line at 32,767 characters. An oversized call dies with `ENAMETOOLONG` before anything runs. This cap sets the inline-write limit in rule `execution`.

## Hooks
- Hooks run through `sh`. Git's `bin` folder (for example `C:\Program Files\Git\bin`) must be on PATH for the harness process. Why: without it, every hook call fails before the hook runs.

## Line endings and encoding
- `.gitattributes` sets the line endings: LF for text, CRLF for `.ps1`, `.cmd` and `.bat`. Do not rely on `core.autocrlf`.
- Normalize CRLF to LF before you match text in code. Why: Windows tools write CRLF, and an anchor with `\n` then finds nothing.
- Write files from Python with `open(path, "w", encoding="utf-8", newline="\n")`. Why: text mode on Windows writes CRLF.
- Run Python scripts with `python -X utf8` when they print non-ASCII text. Why: the default code page can abort the output midway.

## Processes and file locks
- A running `CodeView.exe` locks its file: a release build fails while `target/release/CodeView.exe` runs, and a copy to `dist/` fails while `dist/CodeView.exe` runs. `pwsh -NoProfile -File scripts/ship.ps1` stops every instance, waits for each to exit, then copies and starts the copy (rule `rust`, Ship); copy by hand after a kill only when you wait for the process to exit first.
- A test binary that hangs keeps `target/` locked for the next build. Stop it by process ID, not by name: another worktree may run its own tests.
