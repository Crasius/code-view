# Code View browser extension

Opens pull and merge request links of your Code View connections in Code View instead of the browser, in Chrome and Edge. The design, the install steps and the failure cases are in [`docs/browser-extension.md`](../docs/browser-extension.md).

Code View ships these files inside `CodeView.exe` and unpacks them on **Register** (Settings → Links → Browser extension); load that folder with **Load unpacked** in `chrome://extensions` or `edge://extensions`. This folder is the source.

## Permissions

| Permission | Why |
|---|---|
| `nativeMessaging` | Asks Code View (its native messaging host `de.crasius.codeview`) whether it opens a link, and hands the link over. |
| `storage` | Keeps the popup's on/off switch (`storage.local`) and, for ten seconds, the links you opened with a modifier key so they stay in the browser (`storage.session`). |

No host permissions and no `tabs` permission: the content script runs through its `matches` (`https://*/*`), and closing the tab it reports from needs no permission. The extension loads no remote code and sends nothing anywhere but to Code View on this computer.

## Files

| File | Does |
|---|---|
| `manifest.json` | Manifest V3. `key` pins the id `bjnmgiogdmaelabjgocfnplmmlbpfpog`, which the host manifest allows. |
| `content.js` | At `document_start` in the top frame: a top-level navigation to a pull request address goes to the service worker; on Open the tab goes back or closes. Notes links opened with Ctrl, Shift, Meta, Alt or the middle button. |
| `service_worker.js` | The only part that talks to Code View: checks the sender, then `check` and `open` through `runtime.sendNativeMessage`. |
| `popup.html`, `popup.css`, `popup.js` | Whether Code View answers, and the on/off switch. |
| `icons/` | The app icon at 16, 32, 48 and 128 px. |
