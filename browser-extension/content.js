// Code View: runs at document_start in the top frame of every https page. A top-level navigation
// to something that looks like a pull or merge request goes to the service worker, which asks
// Code View; when Code View opens it, this tab steps back (or closes) instead of loading it.
// Links opened with a modifier key or the middle button, reloads, back and forward, and
// navigation within a site the tab is already on stay in the browser.
(() => {
  'use strict';

  // Any host: Code View's own check decides whether the host is one of the user's connections.
  const PULL_REQUEST_PATH = /\/(pull|-\/merge_requests|pullrequest)\/\d+(\/|$)/i;
  const BROWSING_KEY = 'codeview-browsing';

  function looksLikePullRequest(url) {
    return PULL_REQUEST_PATH.test(new URL(url).pathname);
  }

  // The user is working on this site in this tab: Code View stays out of the way.
  function hasBrowsedHere() {
    const host = location.host;
    let hasBrowsed = false;
    try {
      hasBrowsed = sessionStorage.getItem(BROWSING_KEY) === host;
    } catch {
      // Storage blocked for this site: only the referrer counts.
    }
    const isFromHere = document.referrer !== '' && new URL(document.referrer).host === host;
    return hasBrowsed || isFromHere;
  }

  // Written only when the page stays in the browser, never on a page Code View takes.
  function markBrowsing() {
    try {
      sessionStorage.setItem(BROWSING_KEY, location.host);
    } catch {
      // Storage blocked for this site: nothing to remember.
    }
  }

  function isRevisit() {
    const [entry] = performance.getEntriesByType('navigation');
    return entry !== undefined && (entry.type === 'reload' || entry.type === 'back_forward');
  }

  // Nothing to go back to: a tab opened for this link (from another app, a link with
  // target=_blank), or a new tab the link was typed or pasted into, whose only entry before it
  // is the browser's New Tab page. Stepping back there would leave that empty page behind.
  // Without the tabs permission the entry before is not readable, so a link typed into a tab
  // with one other page closes that tab too (Ctrl+Shift+T restores it).
  function isFirstPageOfTab() {
    return history.length === 1 || (history.length === 2 && document.referrer === '');
  }

  function send(message) {
    return chrome.runtime.sendMessage(message).catch(() => ({}));
  }

  async function handOver() {
    const url = location.href;
    if (!looksLikePullRequest(url)) {
      return;
    }
    if (isRevisit() || hasBrowsedHere()) {
      markBrowsing();
      return;
    }
    const answer = await send({ type: 'navigation', url });
    if (!answer?.open) {
      markBrowsing();
      return;
    }
    window.stop();
    if (isFirstPageOfTab()) {
      await send({ type: 'close' });
    } else {
      history.back();
    }
  }

  // A link opened in a new tab or window on purpose: that navigation stays in the browser once.
  function notePass(event) {
    const isModified = event.ctrlKey || event.shiftKey || event.metaKey || event.altKey;
    const isMiddle = event.type === 'auxclick' && event.button === 1;
    if (!isModified && !isMiddle) {
      return;
    }
    const link = event.target instanceof Element ? event.target.closest('a[href]') : null;
    if (link && link.href.startsWith('https://') && looksLikePullRequest(link.href)) {
      send({ type: 'pass', url: link.href });
    }
  }

  document.addEventListener('click', notePass, true);
  document.addEventListener('auxclick', notePass, true);
  handOver();
})();
