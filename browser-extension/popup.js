// Code View popup: whether the native host answers, and the on/off switch.
'use strict';

const status = document.getElementById('status');
const title = document.getElementById('status-title');
const detail = document.getElementById('status-detail');
const enabled = document.getElementById('enabled');

function show(state, heading, text) {
  status.dataset.state = state;
  title.textContent = heading;
  detail.textContent = text;
}

chrome.runtime.sendMessage({ type: 'ping' }).then(
  (answer) => {
    if (answer && answer.type === 'pong') {
      show('ok', 'Code View answers', `Version ${answer.version}`);
      return;
    }
    show(
      'off',
      'Code View does not answer',
      'In Code View, open Settings, Links, and choose Register under Browser extension.',
    );
  },
  () => show('off', 'Code View does not answer', 'Reload the extension in chrome://extensions.'),
);

chrome.storage.local.get({ enabled: true }).then(({ enabled: isOn }) => {
  enabled.checked = isOn;
});

enabled.addEventListener('change', () => {
  chrome.storage.local.set({ enabled: enabled.checked });
});
