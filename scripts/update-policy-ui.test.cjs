const assert = require('node:assert/strict');
const test = require('node:test');
const { mount, state, button } = require('./ui-harness.cjs');
test('development preview hides updater controls and fails closed for absent policy', async () => {
  const app = await mount(state({ updater_enabled: false }));
  try {
    assert.equal(button(app.document, 'Check for updates'), undefined);
    assert.equal(button(app.document, 'Install update'), undefined);
    assert.match(app.document.body.textContent, /Development \/ preview build.*Updates disabled/);
    assert.deepEqual(app.calls, []);
    await app.emit(state());
    assert.equal(button(app.document, 'Check for updates'), undefined);
    assert.match(app.document.body.textContent, /Updates disabled/);
  } finally { await app.close(); }
});
