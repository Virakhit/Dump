const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');
const React = require('react');
const ts = require('typescript');
const { JSDOM } = require('jsdom');
const pending = { peer_id: 'requester', workspace_id: 'office', name: 'Friend', fingerprint: 'authenticated-requester-fingerprint' };
function state(overrides = {}) {
  return { peer_id: 'owner', fingerprint: 'owner-fingerprint', device_name: 'Owner', active: 'office',
    workspaces: [{ snapshot: { workspace_id: 'office', name: 'Office', owner_peer_id: 'owner', members: [{ peer_id: 'owner', name: 'Owner' }], revision: 1 }, mine: true }],
    files: [], online: [], pending: [], preparing: [], joining: false, network_status: 'LAN discovery is running', transfers: [], chat_messages: [], invite_url: null, last_error: null, ...overrides };
}
async function mount(initial, { failChat = false, onChat } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost' });
  global.window = dom.window; global.document = dom.window.document;
  global.HTMLElement = dom.window.HTMLElement; global.IS_REACT_ACT_ENVIRONMENT = true;
  dom.window.HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  const actions = [], calls = [];
  let current = initial, listener, root;
  const core = { isTauri: () => true, Channel: class {}, invoke: async (command, args) => {
    if (command === 'get_state') return current;
    if (command === 'send_chat') { calls.push({ command, args }); if (failChat) throw new Error('Local message could not be saved'); return onChat ? onChat(args) : 'saved-message-id'; }
    if (command === 'dispatch') {
      actions.push(args.action);
      if (args.action.type === 'create_invite') {
        current = { ...current, invite_url: 'dump://join/test-invitation' };
        listener({ payload: current });
      }
    }
  } };
  const modules = new Map();
  function load(filename) {
    if (modules.has(filename)) return modules.get(filename).exports;
    const module = { exports: {} }; modules.set(filename, module);
    const localRequire = id => {
    if (id.endsWith('.css')) return {};
    if (id === '@tauri-apps/api/core') return core;
    if (id === '@tauri-apps/api/app') return { getVersion: async () => '0.1.1' };
    if (id === '@tauri-apps/api/event') return { listen: async (name, callback) => { if (name === 'dump-state') listener = callback; return () => {}; } };
    if (id === '@tauri-apps/api/webviewWindow') return { getCurrentWebviewWindow: () => ({ onDragDropEvent: async () => () => {} }) };
    if (id === 'react-dom/client') return { createRoot: (...args) => { root = require(id).createRoot(...args); return root; } };
    if (id.startsWith('.')) return load(path.resolve(path.dirname(filename), id + '.tsx'));
    return require(id);
  };
  const code = ts.transpileModule(fs.readFileSync(filename, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true, target: ts.ScriptTarget.ES2022 } }).outputText;
  vm.runInThisContext('(function(require,module,exports){' + code + '\n})', { filename })(localRequire, module, module.exports);
    return module.exports;
  }
  await React.act(async () => load(path.resolve(__dirname, '../src/main.tsx')));
  return { document: dom.window.document, actions, calls,
    async emit(next) { current = next; await React.act(async () => listener({ payload: next })); },
    async click(element) { assert.ok(element, 'expected visible control'); await React.act(async () => element.click()); },
    async input(element, value) { assert.ok(element); await React.act(async () => { const proto = element.tagName === 'TEXTAREA' ? dom.window.HTMLTextAreaElement.prototype : dom.window.HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(element, value); element.dispatchEvent(new dom.window.Event('input', { bubbles: true })); }); },
    async close() { await React.act(async () => root.unmount()); dom.window.close(); }
  };
}
const button = (doc, name) => [...doc.querySelectorAll('button')].find(el => el.textContent.includes(name));

module.exports = { mount, state, pending, button };
