const assert = require('node:assert/strict');
const test = require('node:test');
const React = require('react');
const { mount, state, button } = require('./ui-harness.cjs');
function entry(overrides = {}) {
  return { message: { id: 'm1', workspace_id: 'office', sender_peer_id: 'owner', body: 'Hello', sent_at: 1 }, sender_name: 'Owner', delivery: null, ...overrides };
}

test('chat renders only the active workspace and treats message bodies as plain text', async () => {
  const app = await mount(state({ chat_messages: [entry({ message: { ...entry().message, body: '<img src=x onerror=alert(1)>' } }), entry({ message: { ...entry().message, id: 'm2', workspace_id: 'secret-room', body: 'Private other room' } })] }));
  try {
    await app.click(button(app.document, 'Workspace chat'));
    const panel = app.document.querySelector('#workspace-panel');
    assert.match(panel.textContent, /<img src=x onerror=alert\(1\)>/);
    assert.doesNotMatch(panel.textContent, /Private other room/);
    assert.equal(panel.querySelector('img'), null);
  } finally { await app.close(); }
});

test('local save and partial receipts never imply all devices received or read a message', async () => {
  const app = await mount(state({ chat_messages: [entry({ delivery: { status: 'sending', attempted: 2, acknowledged: 0, offline: 1 } })] }));
  try {
    await app.click(button(app.document, 'Workspace chat'));
    assert.match(app.document.querySelector('#workspace-panel').textContent, /Saved locally.*sending/);
    assert.doesNotMatch(app.document.querySelector('#workspace-panel').textContent, /Read by|Delivered to everyone/);
    await app.emit(state({ chat_messages: [entry({ delivery: { status: 'complete', attempted: 2, acknowledged: 1, offline: 1 } })] }));
    assert.match(app.document.querySelector('#workspace-panel').textContent, /1 of 2.*confirmed storage/);
    assert.match(app.document.querySelector('#workspace-panel').textContent, /1 offline.*not queued/);
    await app.emit(state({ chat_messages: [entry({ delivery: { status: 'complete', attempted: 0, acknowledged: 0, offline: 2 } })] }));
    assert.match(app.document.querySelector('#workspace-panel').textContent, /No other devices online.*not queued/i);
  } finally { await app.close(); }
});

test('chat send uses the captured room, clears only its saved draft and waits for backend history', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Workspace chat'));
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'Hello office');
    await app.click(button(app.document, 'Send message'));
    assert.deepEqual(app.calls, [{ command: 'send_chat', args: { workspace: 'office', body: 'Hello office' } }]);
    assert.equal(app.document.querySelector('textarea[aria-label="Message"]').value, '');
    assert.doesNotMatch(app.document.querySelector('.chat-history').textContent, /Hello office/);
  } finally { await app.close(); }
});

test('failed local chat save retains draft and reports failure without a delivery claim', async () => {
  const app = await mount(state(), { failChat: true });
  try {
    await app.click(button(app.document, 'Workspace chat'));
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'Keep this draft');
    await app.click(button(app.document, 'Send message'));
    assert.equal(app.document.querySelector('textarea[aria-label="Message"]').value, 'Keep this draft');
    assert.match(app.document.querySelector('#workspace-panel').textContent, /Local message could not be saved/);
    assert.doesNotMatch(app.document.querySelector('.chat-history').textContent, /Keep this draft/);
  } finally { await app.close(); }
});

test('chat keeps drafts per room and blocks oversized UTF-8 and revoked-member sends', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Workspace chat'));
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'Office draft');
    const other = state({ active: 'other', workspaces: [{ snapshot: { workspace_id: 'other', name: 'Other', owner_peer_id: 'owner', members: [{ peer_id: 'owner', name: 'Owner' }], revision: 1 }, mine: true }] });
    await app.emit(other);
    assert.equal(app.document.querySelector('textarea[aria-label="Message"]').value, '');
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'Other draft');
    await app.emit(state());
    assert.equal(app.document.querySelector('textarea[aria-label="Message"]').value, 'Office draft');
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), '😀'.repeat(1025));
    assert.ok(button(app.document, 'Send message').disabled);
    await app.emit(state({ peer_id: 'removed-device' }));
    assert.ok(button(app.document, 'Send message').disabled);
    assert.deepEqual(app.calls, []);
  } finally { await app.close(); }
});

test('late save completion preserves edits and the draft in a newly selected workspace', async () => {
  let complete;
  const saved = new Promise(resolve => { complete = resolve; });
  const app = await mount(state(), { onChat: () => saved });
  try {
    await app.click(button(app.document, 'Workspace chat'));
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'First office message');
    await app.click(button(app.document, 'Send message'));
    assert.ok(button(app.document, 'Saving locally').disabled);
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'Next office draft');
    const other = state({ active: 'other', workspaces: [{ snapshot: { workspace_id: 'other', name: 'Other', owner_peer_id: 'owner', members: [{ peer_id: 'owner', name: 'Owner' }], revision: 1 }, mine: true }] });
    await app.emit(other);
    await app.input(app.document.querySelector('textarea[aria-label="Message"]'), 'Other workspace draft');
    await React.act(async () => complete('saved-id'));
    assert.equal(app.document.querySelector('textarea[aria-label="Message"]').value, 'Other workspace draft');
    await app.emit(state());
    assert.equal(app.document.querySelector('textarea[aria-label="Message"]').value, 'Next office draft');
    assert.deepEqual(app.calls, [{ command: 'send_chat', args: { workspace: 'office', body: 'First office message' } }]);
  } finally { complete('saved-id'); await app.close(); }
});
