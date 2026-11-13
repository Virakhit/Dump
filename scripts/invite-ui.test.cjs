const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');
const { createRequire } = require('node:module');
const project = path.resolve(__dirname, '..');
const requireProject = createRequire(path.join(project, 'package.json'));
const ts = requireProject('typescript');
const React = requireProject('react');
const { JSDOM } = requireProject('jsdom');
const workspace = 'team';
const pending = { peer_id: 'member-device', name: 'Joining teammate', workspace_id: workspace, fingerprint: 'verified-test-fingerprint' };
function state(overrides = {}) {
  return { peer_id: 'owner-device', relay_peer_id: 'relay-device', relay_identity_migrated: false, fingerprint: 'owner-fingerprint', device_name: 'Owner', active: workspace,
    workspaces: [{ snapshot: { workspace_id: workspace, name: 'Office', owner_peer_id: 'owner-device', members: [{ peer_id: 'owner-device', name: 'Owner' }], revision: 1 }, mine: true }],
    online: [], files: [], chat_messages: [], pending: [], preparing: [], joining: false, join_stage: null, network_status: 'LAN discovery is running', relay_peers: [], transfers: [], invite_url: null, last_error: null,
    network: { help_network: false, public_addresses: [], relays: [], use_tailscale: false }, tailscale: { state: 'not_installed', listener_ready: false }, network_policy: 'legacy', ...overrides };
}
async function mount(initial, { failAction } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost' });
  global.window = dom.window; global.document = dom.window.document;
  global.HTMLElement = dom.window.HTMLElement; global.IS_REACT_ACT_ENVIRONMENT = true;
  dom.window.HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  dom.window.HTMLDialogElement.prototype.close = function () { this.open = false; };
  let current = initial, stateListener, root;
  const actions = [], calls = [];
  const api = {
    isTauri: () => true, Channel: class {},
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === 'get_state') return current;
      if (command === 'dispatch') {
        actions.push(args.action);
        if (args.action.type === failAction) throw new Error('Request could not be processed');
        if (['create_lan_invite', 'create_invite', 'create_overlay_invite', 'create_overlay_connect'].includes(args.action.type)) {
          current = { ...current, invite_url: args.action.type.startsWith('create_overlay') ? 'dump://tailscale/BBBBBBBB' : args.action.type === 'create_invite' ? 'dump://join/CCCCCCCC' : 'dump://lan/AAAAAAAAAAAAAAAA' };
          stateListener({ payload: current });
        }
      }
    }
  };
  const modules = new Map();
  function load(filename) {
    if (modules.has(filename)) return modules.get(filename).exports;
    const module = { exports: {} }; modules.set(filename, module);
    const code = ts.transpileModule(fs.readFileSync(filename, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true, target: ts.ScriptTarget.ES2022 } }).outputText;
    const localRequire = id => {
      if (id.endsWith('.css')) return {};
      if (id === '@tauri-apps/api/core') return api;
      if (id === '@tauri-apps/api/app') return { getVersion: async () => '0.1.2' };
      if (id === '@tauri-apps/api/event') return { listen: async (name, callback) => { if (name === 'dump-state') stateListener = callback; return () => {}; } };
      if (id === '@tauri-apps/api/webviewWindow') return { getCurrentWebviewWindow: () => ({ onDragDropEvent: async () => () => {} }) };
      if (id === 'react-dom/client') { const client = requireProject(id); return { ...client, createRoot: (...args) => { root = client.createRoot(...args); return root; } }; }
      if (id.startsWith('.')) return load(path.resolve(path.dirname(filename), id + '.tsx'));
      return requireProject(id);
    };
    vm.runInThisContext('(function(require,module,exports){' + code + '\n})', { filename })(localRequire, module, module.exports);
    return module.exports;
  }
  await React.act(async () => { load(path.join(project, 'src/main.tsx')); });
  return {
    document: dom.window.document, actions, calls,
    async change(element, value) { assert.ok(element); await React.act(async () => { element.value = value; element.dispatchEvent(new dom.window.Event('change', { bubbles: true })); }); },
    async emit(next) { current = next; await React.act(async () => stateListener({ payload: next })); },
    async click(element) { assert.ok(element, 'expected a visible control'); await React.act(async () => element.click()); },
    async close() { await React.act(async () => root.unmount()); dom.window.close(); }
  };
}
const button = (doc, text) => [...doc.querySelectorAll('button')].find(el => el.textContent.includes(text));
const ready = { state: 'ready', listener_ready: true };
const transfer = { id: 'tx', name: 'large.zip', direction: 'Receiving', peer_id: 'friend', relayed: false, tailscale: true, bytes: 0, total: 1000, status: 'Receiving', error: null };
const checkbox = (doc, text) => [...doc.querySelectorAll('label')].find(el => el.textContent.includes(text))?.querySelector('input');

test('Network settings enables installed Tailscale and refreshes sanitized status', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Network settings'));
    const dialog = app.document.querySelector('dialog');
    assert.match(dialog.textContent, /not installed/i);
    await app.click(checkbox(dialog, 'Use installed Tailscale'));
    await app.click(button(dialog, 'Check Tailscale'));
    assert.ok(app.calls.some(c => c.command === 'refresh_tailscale' && c.args === undefined));
    await app.click(button(dialog, 'Save settings'));
    assert.deepEqual(app.calls.find(c => c.command === 'save_network').args, { settings: { help_network: false, public_addresses: [], relays: [], use_tailscale: true } });
    assert.match(dialog.textContent, /Tailscale changes apply immediately/);
    assert.match(dialog.textContent, /separate Tailscale app/i);
    await app.emit(state({ tailscale: { state: 'ready', listener_ready: false } }));
    assert.match(dialog.textContent, /listener.*not ready/i);
  } finally { await app.close(); }
});

test('workspace policy requires explicit selection and waits for unfinished transfers', async () => {
  const app = await mount(state({ tailscale: ready }));
  try {
    await app.click(button(app.document, 'Network settings'));
    const policy = app.document.querySelector('select[aria-label="Workspace network policy"]');
    assert.equal(app.actions.length, 0);
    await app.change(policy, 'tailscale_only');
    assert.deepEqual(app.actions, [{ type: 'set_network_policy', policy: 'tailscale_only' }]);
    await app.emit(state({ tailscale: ready, transfers: [transfer] }));
    assert.ok(policy.disabled);
    assert.ok(checkbox(app.document, 'Use installed Tailscale').disabled);
    assert.match(app.document.querySelector('dialog').textContent, /Finish.*transfers.*before changing/i);
    assert.equal(app.actions.some(a => a.type === 'cancel_transfer'), false);
  } finally { await app.close(); }
});

test('Tailscale invitation preserves fingerprint approval and explains machine sharing', async () => {
  const app = await mount(state({ tailscale: ready }));
  try {
    await app.click(button(app.document, 'Invite a friend'));
    assert.deepEqual(app.actions[0], { type: 'create_lan_invite' });
    await app.click(button(app.document.querySelector('dialog'), 'Invite via Tailscale'));
    assert.deepEqual(app.actions.at(-1), { type: 'create_overlay_invite' });
    const dialog = app.document.querySelector('dialog');
    assert.equal(dialog.querySelector('textarea').value, 'dump://tailscale/BBBBBBBB');
    assert.match(dialog.textContent, /support.*Tailscale|Tailscale.*support/i);
    assert.match(dialog.textContent, /machine sharing.*Tailscale/i);
    await app.emit(state({ tailscale: ready, pending: [pending], invite_url: 'dump://tailscale/BBBBBBBB' }));
    assert.ok(button(dialog, 'Approve device').disabled);
    await app.click(dialog.querySelector('.request-card input'));
    await app.click(button(dialog, 'Approve device'));
    assert.deepEqual(app.actions.at(-1), { type: 'approve', peer_id: pending.peer_id });
  } finally { await app.close(); }
});

test('Tailscale-only workspace routes default invitation to overlay', async () => {
  const app = await mount(state({ tailscale: ready, network_policy: 'tailscale_only' }));
  try {
    await app.click(button(app.document, 'Invite a friend'));
    assert.deepEqual(app.actions, [{ type: 'create_overlay_invite' }]);
    assert.equal(button(app.document.querySelector('dialog'), 'Use a short link for this network'), undefined);
  } finally { await app.close(); }
});

test('approved non-owner creates connection link while owner offline without inviting members', async () => {
  const app = await mount(state({ peer_id: 'member-device', tailscale: ready, workspaces: [{ mine: false, snapshot: { ...state().workspaces[0].snapshot, members: [{ peer_id: 'owner-device', name: 'Owner' }, { peer_id: 'member-device', name: 'Member' }] } }] }));
  try {
    assert.equal(button(app.document, 'Invite a friend'), undefined);
    await app.click(button(app.document, 'Network settings'));
    await app.click(button(app.document, 'Copy Tailscale connection link'));
    assert.deepEqual(app.actions, [{ type: 'create_overlay_connect' }]);
    const dialog = app.document.querySelector('dialog');
    assert.match(dialog.textContent, /existing members/i);
    assert.equal(dialog.querySelector('textarea').value, 'dump://tailscale/BBBBBBBB');
    assert.equal(button(dialog, 'Invite via Tailscale'), undefined);
  } finally { await app.close(); }
});

test('overlay transfers show Via Tailscale without Direct claim or Dump relay limits', async () => {
  const app = await mount(state({ transfers: [{ ...transfer, relayed: true }] }));
  try {
    assert.match(app.document.querySelector('.transfer').textContent, /Via Tailscale/);
    assert.doesNotMatch(app.document.querySelector('.transfer').textContent, /Via relay|Direct|DERP/);
    assert.doesNotMatch(app.document.querySelector('main').textContent, /Remaining capacity is unknown/);
  } finally { await app.close(); }
});

test('owner sees and reviews an incoming request from Files without automatic approval', async () => {
  const app = await mount(state());
  try {
    await app.emit(state({ pending: [pending] }));
    assert.ok(button(app.document, 'Review requests'), 'incoming request is hidden on the default Files tab');
    assert.equal(app.actions.length, 0, 'arrival must never approve or change workspace');
    await app.click(button(app.document, 'Review requests'));
    assert.equal(app.document.querySelector('#members-tab').getAttribute('aria-selected'), 'true');
    const approve = button(app.document, 'Approve device');
    assert.ok(approve.disabled, 'fingerprint verification is still required');
    await app.click(app.document.querySelector('.request-card input[type=checkbox]'));
    await app.click(button(app.document, 'Approve device'));
    assert.deepEqual(app.actions, [{ type: 'approve', peer_id: pending.peer_id }]);
  } finally { await app.close(); }
});

test('owner can review a request while the invite dialog is still open', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Invite a friend'));
    await app.emit(state({ pending: [pending], invite_url: 'dump://lan/AAAAAAAAAAAAAAAA' }));
    const dialog = app.document.querySelector('dialog[open]');
    assert.ok(dialog);
    await app.click(button(dialog, 'Review requests'));
    assert.equal(app.document.querySelector('dialog[open]'), null);
    assert.equal(app.document.querySelector('#members-tab').getAttribute('aria-selected'), 'true');
    assert.ok(button(app.document, 'Approve device').disabled);
  } finally { await app.close(); }
});

test('owner can approve directly inside the invitation dialog after checking the fingerprint', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Invite a friend'));
    await app.emit(state({ pending: [pending], invite_url: 'dump://lan/AAAAAAAAAAAAAAAA' }));
    const dialog = app.document.querySelector('dialog[open]');
    const approve = button(dialog, 'Approve device');
    assert.ok(approve, 'approval must be available inside the open invitation dialog');
    assert.ok(approve.disabled);
    assert.match(dialog.textContent, /verified-test-fingerprint/);
    assert.equal(app.actions.some(action => action.type === 'approve'), false);
    await app.click(dialog.querySelector('.request-card input[type=checkbox]'));
    await app.click(button(dialog, 'Approve device'));
    assert.deepEqual(app.actions.at(-1), { type: 'approve', peer_id: pending.peer_id });
    assert.ok(!app.document.querySelector('dialog[open]'), 'a consumed invitation must not remain available to copy');
    await app.emit(state({ invite_url: 'dump://lan/AAAAAAAAAAAAAAAA' }));
    assert.equal(button(app.document, 'Approve device'), undefined);
  } finally { await app.close(); }
});

test('rejecting inside the invitation dialog hides the revoked link', async () => {
  const app = await mount(state());
  try {
    await app.click(button(app.document, 'Invite a friend'));
    await app.emit(state({ pending: [pending], invite_url: 'dump://lan/AAAAAAAAAAAAAAAA' }));
    await app.click(button(app.document.querySelector('dialog[open]'), 'Reject'));
    assert.deepEqual(app.actions.at(-1), { type: 'reject', peer_id: pending.peer_id });
    assert.ok(!app.document.querySelector('dialog[open]'), 'a revoked invitation must not remain available to copy');
  } finally { await app.close(); }
});

test('a failed rejection keeps the request dialog open for retry', async () => {
  const app = await mount(state(), { failAction: 'reject' });
  try {
    await app.click(button(app.document, 'Invite a friend'));
    await app.emit(state({ pending: [pending], invite_url: 'dump://lan/AAAAAAAAAAAAAAAA' }));
    await app.click(button(app.document.querySelector('dialog[open]'), 'Reject'));
    const dialog = app.document.querySelector('dialog[open]');
    assert.ok(dialog);
    assert.match(dialog.textContent, /Request could not be processed/);
    assert.ok(button(dialog, 'Reject'));
  } finally { await app.close(); }
});

test('owner Members explains when no join request has arrived', async () => {
  const app = await mount(state());
  try {
    await app.click(app.document.querySelector('#members-tab'));
    assert.match(app.document.querySelector('#workspace-panel').textContent, /No join requests have reached this workspace/);
    assert.equal(button(app.document, 'Approve device'), undefined);
    await app.emit(state({ pending: [pending] }));
    assert.doesNotMatch(app.document.querySelector('#workspace-panel').textContent, /No join requests have reached this workspace/);
    assert.ok(button(app.document, 'Approve device').disabled);
  } finally { await app.close(); }
});

test('incoming requests remain visible from Chat and disappear when the owner list clears', async () => {
  const app = await mount(state());
  try {
    await app.click(app.document.querySelector('#chat-tab'));
    await app.emit(state({ pending: [pending] }));
    assert.ok(button(app.document, 'Review requests'));
    assert.equal(app.document.querySelector('#chat-tab').getAttribute('aria-selected'), 'true', 'arrival must not interrupt chat');
    await app.emit(state());
    assert.equal(button(app.document, 'Review requests'), undefined);
  } finally { await app.close(); }
});

test('only the active workspace owner sees request controls', async () => {
  const nonOwner = state({ peer_id: 'member-device', pending: [pending], workspaces: [{ ...state().workspaces[0], mine: false }] });
  const app = await mount(nonOwner);
  try {
    assert.equal(button(app.document, 'Review requests'), undefined);
    await app.emit(state({ pending: [{ ...pending, workspace_id: 'other-workspace' }] }));
    assert.equal(button(app.document, 'Review requests'), undefined);
  } finally { await app.close(); }
});

test('invitee is not told approval is pending before a request has reached the owner', async () => {
  const app = await mount(state({ joining: true, join_stage: 'finding_owner' }));
  try {
    assert.match(app.document.querySelector('main').textContent, /Finding the workspace owner/);
    assert.doesNotMatch(app.document.querySelector('main').textContent, /Waiting for the workspace owner/);
    await app.emit(state({ joining: true, join_stage: 'awaiting_approval' }));
    assert.match(app.document.querySelector('main').textContent, /Request received by the workspace owner/);
  } finally { await app.close(); }
});
