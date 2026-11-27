// Code View: the only part of the extension that talks to the native host. Content scripts and
// the popup send it messages; it checks who sent them, forwards a fixed set of requests to
// Code View and answers with what Code View said.
'use strict';

const HOST = 'de.crasius.codeview';
// A link opened with a modifier key stays in the browser once, for this long.
const PASS_MS = 10000;
// The app's single-instance line limit (docs/browser-extension.md, Single instance).
const MAX_URL = 8192;

async function askHost(request) {
  try {
    return (await chrome.runtime.sendNativeMessage(HOST, request)) ?? { type: 'error', message: 'No answer' };
  } catch (error) {
    return { type: 'error', message: error.message };
  }
}

async function isEnabled() {
  const { enabled } = await chrome.storage.local.get({ enabled: true });
  return enabled;
}

// Every read-modify-write of the passes runs after the one before, so none overwrites another.
let passQueue = Promise.resolve();

function withPasses(update) {
  const next = passQueue.then(update);
  passQueue = next.catch(() => {});
  return next;
}

// Addresses the user opened with Ctrl, Shift, Meta, Alt or the middle button, with when.
function takePass(url) {
  return withPasses(async () => {
    const { passes = {} } = await chrome.storage.session.get('passes');
    const now = Date.now();
    const kept = Object.fromEntries(Object.entries(passes).filter(([, at]) => now - at < PASS_MS));
    const hasPass = url in kept;
    delete kept[url];
    await chrome.storage.session.set({ passes: kept });
    return hasPass;
  });
}

function addPass(url) {
  return withPasses(async () => {
    const { passes = {} } = await chrome.storage.session.get('passes');
    passes[url] = Date.now();
    await chrome.storage.session.set({ passes });
  });
}

function isHttpsUrl(value) {
  try {
    return typeof value === 'string' && value.length <= MAX_URL && new URL(value).protocol === 'https:';
  } catch {
    return false;
  }
}

// A navigation the content script reports: Code View opens it, or the page stays.
async function navigation(url) {
  if (!(await isEnabled()) || (await takePass(url))) {
    return { open: false };
  }
  const check = await askHost({ type: 'check', url });
  // Only `open` leaves the browser. `pass` is a page Code View itself just opened here, which
  // must not bounce back; every other state (no access, not connected, errors) stays too.
  if (check.state !== 'open') {
    return { open: false };
  }
  const opened = await askHost({ type: 'open', url });
  return { open: opened.state === 'forwarded' || opened.state === 'started' };
}

async function handle(message, sender) {
  const fromTab = sender.tab && sender.frameId === 0;
  switch (message && message.type) {
    case 'navigation':
      return fromTab && isHttpsUrl(message.url) && message.url === sender.url
        ? navigation(message.url)
        : { open: false };
    case 'pass':
      if (fromTab && isHttpsUrl(message.url)) {
        await addPass(message.url);
      }
      return {};
    case 'close':
      if (fromTab) {
        await chrome.tabs.remove(sender.tab.id);
      }
      return {};
    case 'ping':
      // The popup, an extension page: no tab.
      return sender.tab ? {} : askHost({ type: 'ping' });
    default:
      return { error: 'unknown request' };
  }
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  handle(message, sender).then(sendResponse, (error) => sendResponse({ error: String(error) }));
  return true;
});
