const assert = require('node:assert/strict');
const test = require('node:test');
const { mount, state, pending, button } = require('./ui-harness.cjs');

test('shared invitation explains separate approvals and can be revoked without claiming Tailscale connected', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Invite a friend'));
    await app.emit(state({ invite_url: 'dump://connect/signed-test-envelope', invite_note: 'Tailscale could not be confirmed on this device. This link can still use LAN discovery.' }));
    const dialog = app.document.querySelector('dialog[open]');
    assert.match(dialog.textContent, /Approve each device separately/);
    assert.match(dialog.textContent, /Tailscale could not be confirmed/);
    assert.doesNotMatch(dialog.textContent, /Connected via Tailscale/);
    await app.click(button(dialog, 'Revoke invitation'));
    assert.deepEqual(app.actions.at(-1), { type: 'revoke_invite' });
    assert.ok(!app.document.querySelector('dialog[open]'));
  } finally { await app.close(); }
});

test('join stages distinguish route attempts from the authenticated owner acknowledgment', async () => {
  const app = await mount(state({ joining: true, join_stage: 'finding_owner', join_owner_fingerprint: 'owner-pin' }));
  try {
    assert.match(app.document.querySelector('main').textContent, /Finding the invitation owner/);
    assert.doesNotMatch(app.document.querySelector('main').textContent, /Request received by the workspace owner/);
    await app.emit(state({ joining: true, join_stage: 'connecting', join_owner_fingerprint: 'owner-pin' }));
    assert.match(app.document.querySelector('main').textContent, /Trying the invitation owner/);
    assert.match(app.document.querySelector('main').textContent, /owner-pin/);
    await app.emit(state({ joining: true, join_stage: 'awaiting_approval' }));
    assert.match(app.document.querySelector('main').textContent, /Request received by the workspace owner/);
  } finally { await app.close(); }
});

test('owner can confirm zero received requests in Members, then sees authenticated arrival', async () => {
  const app = await mount(state());
  try {
    await app.click(app.document.querySelector('#members-tab'));
    assert.match(app.document.querySelector('main').textContent, /No join requests received for this workspace/);
    assert.ok(!button(app.document, 'Approve device'), 'approval must not be available');
    await app.emit(state({ pending: [pending] }));
    assert.doesNotMatch(app.document.querySelector('main').textContent, /No join requests received for this workspace/);
    assert.match(app.document.querySelector('main').textContent, /1 request received on this device/);
    assert.ok(button(app.document, 'Approve device').disabled);
    assert.deepEqual(app.actions, []);
  } finally { await app.close(); }
});

test('owner can reach the fingerprint gate from Files after receiving a request', async () => {
  const app = await mount(state());
  try {
    await app.emit(state({ pending: [pending] }));
    await app.click(button(app.document, 'Review requests'));
    assert.equal(app.document.querySelector('#members-tab').getAttribute('aria-selected'), 'true');
    assert.match(app.document.querySelector('.request-card').textContent, /authenticated-requester-fingerprint/);
    assert.ok(button(app.document, 'Approve device').disabled);
    assert.deepEqual(app.actions, []);
    await app.click(app.document.querySelector('.request-card input[type=checkbox]'));
    await app.click(button(app.document, 'Approve device'));
    assert.deepEqual(app.actions, [{ type: 'approve', peer_id: 'requester' }]);
  } finally { await app.close(); }
});

test('owner can review arrival from the invitation dialog without changing workspace', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Invite a friend'));
    let dialog = app.document.querySelector('dialog[open]');
    assert.match(dialog.textContent, /No join requests received for this workspace/);
    await app.emit(state({ pending: [pending], invite_url: 'dump://join/test-invitation' }));
    dialog = app.document.querySelector('dialog[open]');
    assert.match(dialog.textContent, /1 request received on this device/);
    await app.click(button(dialog, 'Review requests'));
    assert.ok(!app.document.querySelector('dialog[open]'), 'review closes the dialog');
    assert.equal(app.document.querySelector('#members-tab').getAttribute('aria-selected'), 'true');
    assert.ok(button(app.document, 'Approve device').disabled);
    assert.deepEqual(app.actions, [{ type: 'create_invite' }]);
  } finally { await app.close(); }
});

test('only the selected workspace owner receives diagnostics and approval controls', async () => {
  const app = await mount(state({ pending: [{ ...pending, workspace_id: 'other-room' }] }));
  try {
    assert.ok(!button(app.document, 'Review requests'), 'review must not be available');
    await app.click(app.document.querySelector('#members-tab'));
    assert.ok(!button(app.document, 'Approve device'), 'approval must not be available');
    await app.emit(state({ peer_id: 'member', pending: [pending], workspaces: [{ ...state().workspaces[0], mine: false }] }));
    assert.ok(!button(app.document, 'Review requests'), 'review must not be available');
    assert.ok(!button(app.document, 'Approve device'), 'approval must not be available');
    assert.doesNotMatch(app.document.querySelector('main').textContent, /No join requests received for this workspace/);
    assert.deepEqual(app.actions, []);
  } finally { await app.close(); }
});

test('joining and discovery alone do not claim that an owner received a request', async () => {
  const app = await mount(state({ joining: true }));
  try {
    const notice = [...app.document.querySelectorAll('.notice')].find(el => button(el, 'Cancel'));
    assert.ok(notice);
    assert.match(notice.textContent, /Joining the workspace/);
    assert.doesNotMatch(notice.textContent, /Waiting for the workspace owner|Request received by/);
    assert.match(notice.textContent, /confirm the request arrived/);
    await app.click(button(notice, 'Cancel'));
    assert.deepEqual(app.actions, [{ type: 'cancel_join' }]);
    await app.emit(state({ joining: false }));
    assert.equal(button(app.document, 'Cancel'), undefined);
  } finally { await app.close(); }
});

test('request receipt diagnostic disappears when pending state is cleared', async () => {
  const app = await mount(state({ pending: [pending] }));
  try {
    assert.ok(button(app.document, 'Review requests'));
    await app.emit(state());
    assert.ok(!button(app.document, 'Review requests'), 'review must not be available');
    assert.doesNotMatch(app.document.querySelector('main').textContent, /1 request received on this device/);
  } finally { await app.close(); }
});

test('review from invitation dialog works even when Members was already selected', async () => {
  const app = await mount(state({ pending: [pending] }));
  try {
    await app.click(app.document.querySelector('#members-tab'));
    await app.click(button(app.document, 'Invite a friend'));
    const dialog = app.document.querySelector('dialog[open]');
    await app.click(button(dialog, 'Review requests'));
    assert.ok(!app.document.querySelector('dialog[open]'));
    assert.ok(button(app.document, 'Approve device').disabled);
    assert.deepEqual(app.actions, [{ type: 'create_invite' }]);
  } finally { await app.close(); }
});
