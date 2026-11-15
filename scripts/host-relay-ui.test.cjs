const assert = require('node:assert/strict');
const test = require('node:test');
const { mount, state, button } = require('./ui-harness.cjs');
test('only owner can enable host relay and online-only limits remain explicit', async () => {
  const app = await mount(state());
  try {
    assert.match(app.document.body.textContent, /Both the host and source must stay online/);
    await app.click(button(app.document, 'Enable host relay'));
    assert.deepEqual(app.actions, [{ type: 'set_host_relay', enabled: true }]);
    await app.emit(state({ peer_id: 'member', host_relay_ready: false }));
    assert.equal(button(app.document, 'Enable host relay'), undefined);
    assert.match(app.document.body.textContent, /Host relay unavailable/);
    await app.emit(state({ peer_id: 'member', host_relay_ready: true }));
    assert.match(app.document.body.textContent, /Host relay last confirmed ready/);
    assert.doesNotMatch(app.document.body.textContent, /Source files cached/);
  } finally { await app.close(); }
});
test('relayed transfer identifies the actual route without claiming completion', async () => {
  const app = await mount(state({ transfers: [{ id: 't', name: 'file', direction: 'Receiving', status: 'Receiving', total: 3, bytes: 1, via_host: true }] }));
  try {
    assert.match(app.document.querySelector('.transfer-title').textContent, /Via workspace host/);
    assert.doesNotMatch(app.document.querySelector('.transfer-title').textContent, /Completed/);
  } finally { await app.close(); }
});
