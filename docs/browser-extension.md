# Browser extension

A Chromium extension (Chrome and Edge, Manifest V3) in `browser-extension/` opens pull request links of a connected host in Code View instead of the browser. Three parts work together:

1. **The extension** notices a top-level navigation to an address that looks like a pull or merge request and asks the native host whether Code View opens it.
2. **The native messaging host** is `CodeView.exe` itself, started by the browser. It answers from the saved connections and hands the link to the app.
3. **The single instance** link: a second start of Code View passes its link to the running window and exits.

```
page (content.js) --runtime message--> service worker --sendNativeMessage--> CodeView.exe (host mode)
                                                                                  |
                                                                       local IPC  v
                                                                         running Code View window
```

## Install (owner's steps)

1. In Code View: Settings → Links → Browser extension → **Register**. This writes the host manifest and the browser's registration for the current user, and unpacks the extension into the app's cache folder.
2. **Open extension folder** shows that folder.
3. In `chrome://extensions` or `edge://extensions`: turn on **Developer mode**, choose **Load unpacked** and pick the folder.

The extension's id is pinned by the `key` in `manifest.json` (`bjnmgiogdmaelabjgocfnplmmlbpfpog`), so the host manifest's `allowed_origins` names it before the extension is loaded. The private key was not kept: an unpacked extension needs only the public key, and nobody packs a `.crx` from it.

After moving or replacing `CodeView.exe`, the row says **Outdated** and **Update** points the registration at the new file.

## Native messaging host

- **Start.** The browser runs the manifest's `path` with the caller's origin as the first argument (`chrome-extension://<id>/`), on Windows followed by `--parent-window=<handle>` [docs: developer.chrome.com native messaging]. `main` recognises that origin before anything else and serves stdin/stdout without a window. `CodeView --native-messaging <origin>` is the same mode by hand, for checks from a shell.
- **Origin.** The host answers only the pinned extension's origin; any other origin gets an `error` answer. The browser already refuses origins that `allowed_origins` lacks; the check keeps the host honest when it is run by hand.
- **Framing.** Each message is UTF-8 JSON preceded by its length as a 32-bit integer in native byte order (little-endian on every system Code View ships for) [docs]. The browser sends at most 64 MiB and accepts at most 1 MiB [docs]; our own limit is 64 KiB per request, since a request carries one URL. A longer or cut message is answered with `error` and the host exits; one that is not JSON or names an unknown `type` gets `error` and the host reads on. Rust's standard streams are binary on Windows, so no `O_BINARY` switch is needed.
- **Lifetime.** The extension uses `runtime.sendNativeMessage`, so the browser starts one host process per message and reads only its first answer [docs]. The host still loops until stdin closes, so `connectNative` would work too.
- **Requests** (field `type`):

| Request | Answer |
|---|---|
| `{"type":"ping"}` | `{"type":"pong","version":"<app version>"}`; the popup's "Code View answers" |
| `{"type":"check","url":...}` | `{"type":"check","state":S}` with `S` one of `pass` (Code View itself just opened this pull request in the browser; see below), `open`, `no_access` (with `denial`: `token_refused` for 401, `forbidden` for 403, `not_found` for 404), `not_connected`, `not_a_pull_request`, `unavailable` (the connections file could not be read; `message` says why) |
| `{"type":"open","url":...}` | `{"type":"open","state":"forwarded"}` (the running window took it), `"started"` (Code View was not running and starts with the link), `not_connected` or `not_a_pull_request` when no connection serves the link; `error` with `message` when neither worked |

- **The answer to `check`** takes the two steps of a copied link (`cv_shell::links::link_access`): `locate` finds the connection, then the host reads the pull request's detail once with that connection's token, within the same budget (`link_access::BUDGET`), and `link_access::verdict` maps the answer (`cv_shell::browser::link_check::check`). Only 401, 403 and 404 are `no_access`; a 5xx, a network failure or no answer in time is `open`, as for a copied link, and the page shows the failure with its own Retry. `not_connected` is a pull request on a platform's cloud host without a connection, `not_a_pull_request` everything else; neither costs a read.
- **`pass`: links the app opens itself.** "Open on GitHub" and every other `ctx.open_url` of a pull request page would otherwise come straight back: the extension sees the navigation, `check` says `open`, and the page returns to the window it came from. After each frame the app reads the frame's `OutputCommand::OpenUrl` commands in one place (`ReviewApp::ui`, `cv_shell::browser::passes::Recorder`), and every URL that `link_access::locate` resolves to a pull request is recorded in `browser/passes.json` in the cache folder, keyed by connection, repository and number, so `/files`, a fragment and the `visualstudio.com` host match too. The write runs on the background runtime through a temp file and a rename, one write at a time, and drops entries older than the TTL. `check` asks the list right after `locate` and answers `pass` within the TTL, without a read; the extension treats `pass` like every state other than `open` and keeps the page. The TTL is 60 seconds, our own budget (`passes::TTL`): the browser starts loading within a second of the request, and a minute also covers a browser that has to start first. No URL is changed (no marker in query or fragment). Without the extension nothing reads the list; a missing or unreadable list is an empty one.
- **`open` does not read again.** It only locates the link and hands it over: the extension sends `open` right after a `check` that said `open`, and the app checks every forwarded link itself before its page opens (below), so a second read in the host would only add a round trip.
- **Connections and tokens** load as the app loads them (`ConnectionStore::open` on `AppPaths::from_environment`). Nothing the host writes to stdout or stderr carries a token: answers carry states and fixed messages, and an error names the file, not its content.

## Single instance

- **Endpoint.** Windows: the named pipe `\\.\pipe\CodeView-<hash>`, where `<hash>` is `stable_hash` of the config folder, so each user (and each data folder) has its own instance. macOS and Linux: the Unix socket `ipc/codeview.sock` in the cache folder; `ipc/` is created with mode 0700.
- **Same user only.** The pipe is created with the DACL `D:P(A;;GA;;;OW)` (full access for the owner only, nothing for anyone else) and `PIPE_REJECT_REMOTE_CLIENTS`, the first instance also with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so nobody can take the name over while we run. Clients open it at identification level (`SECURITY_IDENTIFICATION`), so a server can never act as them. An elevated Code View owns its pipe as Administrators, so a start that is not elevated cannot reach it and opens a window of its own [untested]. The socket's folder is readable and writable by its owner only.
- **Protocol.** The server writes its process id and a newline; the client lets that process take the foreground (`AllowSetForegroundWindow` on Windows, since the user just asked for it), writes the URL and a newline (at most 8 KiB), and reads `ok`. The listener runs on its own thread and blocks in accept, so it costs nothing while idle; each connection is read on a short-lived thread of its own, so a client that stalls holds up nobody.
- **Start.** `main` tries to become the server before the window opens. When a server answers, the link (if any) goes to it and the process exits; without a link the running window comes to the front. A stale socket file (the last instance crashed) is removed when connecting to it is refused.
- **Delivery.** The listener queues the link in the egui context and asks for a repaint, plus `Minimized(false)` and `Focus`; this start's own command-line link takes the same queue (`single_instance::queue_link`). The app takes the queue in its frame (`single_instance::ForwardedLinks`): a pull request of a saved connection starts the access check of a copied link on the background runtime and opens once its token read it, whose detail its page then reuses; a refused one opens no page and gets the copied link's toast with the page on its platform and the token setup. Any other link opens like a typed one (`ReviewApp::open_link`), whose error offers the connection setup or the browser. Nothing on the UI thread waits on the pipe or the host.

## Registration

- **Windows.** The host manifest is `browser/de.crasius.codeview.json` in the cache folder. The default value of `HKCU\Software\Google\Chrome\NativeMessagingHosts\de.crasius.codeview` and of `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\de.crasius.codeview` names it [docs]. Edge falls back to Chrome's key only when its own is missing [docs: learn.microsoft.com Edge native messaging], so both are written.
- **macOS.** `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/de.crasius.codeview.json` and `~/Library/Application Support/Microsoft Edge/NativeMessagingHosts/…` [docs].
- **Linux.** `~/.config/google-chrome/NativeMessagingHosts/` and `~/.config/microsoft-edge/NativeMessagingHosts/` [docs].
- **Status.** Registered: every browser's registration names a manifest whose `path` is the running executable and whose `allowed_origins` holds the extension. Not registered: none does. Outdated: some do, or one names another executable. The status is read on the background runtime when the row is first shown and after every change.
- **Unregister** removes the registry keys or manifest files (and the Windows manifest); the unpacked extension stays, since the browser still has it loaded.
- **Tests** inject the writer: the Windows key store is a fake map, the manifest folders are a scratch folder; no test touches the real registry or the user's browser folders.

## The extension

- **Permissions:** `nativeMessaging` (talk to Code View) and `storage` (the on/off switch, the modifier-click passes). No host permissions, no `tabs`: closing the tab a content script reports from needs none [docs: chrome.tabs.remove], and the content script runs on every https page through its `matches`. No remote code. The service worker answers only its own extension's pages and forwards only a navigation whose URL is the sending tab's own address.
- **When it asks.** `content.js` runs at `document_start` in the top frame of every https page. It asks only when the address looks like a pull or merge request on any host (`/pull/<n>`, `/-/merge_requests/<n>`, `/pullrequest/<n>`); the native host decides whether that host is one of the user's connections.
- **What stays in the browser.**
  - The switch in the popup is off.
  - The navigation came from the same host (`document.referrer`), or the tab has browsed that host before (a `sessionStorage` mark): the user is working on the site.
  - The link was opened with Ctrl, Shift, Meta or Alt, or the middle button: the content script of the page with the link tells the service worker, which lets the next navigation to that address pass once (kept 10 seconds in `storage.session`).
  - The page was reloaded or reached through back and forward (`performance` navigation type).
- **What it does on `open`.** It stops the page loading, sends `open`, then asks the service worker to close the tab when the link is its first page, and otherwise goes back. First page means `history.length` is 1 (a tab opened for the link), or 2 with no referrer: a link typed or pasted into a new tab has only the browser's New Tab page before it, and going back there left that empty tab behind. The page before is not readable without the `tabs` permission, so a link typed into a tab with exactly one other page closes that tab too; Ctrl+Shift+T restores it.
- **The popup** shows whether Code View answers (`ping`), with the fix when it does not (register in Code View's Settings), and the on/off switch.

## Failure cases

| Case | What happens |
|---|---|
| Host not registered | `sendNativeMessage` fails ("Specified native messaging host not found" [docs]); the page loads in the browser, the popup says Code View does not answer and how to register it. |
| App not running | `open` starts `CodeView.exe <url>` from the host; the new window opens the link. |
| Link of no connection | `check` answers `not_connected` or `not_a_pull_request`; the page stays in the browser. |
| Token cannot read the pull request | `check` answers `no_access` with its `denial`; the page stays in the browser, where the user may well be signed in with more rights. |
| Connections unreadable (token store locked) | `check` answers `unavailable`; the page stays. |
| Host moved | The browser cannot start it; the page stays, the Settings row says Outdated with Update. |
| Pipe busy | The client retries for up to two seconds, then starts a new instance as if none ran. |

NOT-VERIFIED: loading the unpacked extension in a real browser profile and the tab handling there; the macOS and Linux paths beyond compiling them.
