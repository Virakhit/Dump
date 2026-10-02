const { test } = require('node:test');
const assert = require('node:assert/strict');
const publish = require('./publish-update.cjs');

test('the update channel announces complete signed releases and never moves backwards', async () => {
  const originalFetch = global.fetch;
  const manifest = { version: '0.1.1', platforms: { 'windows-x86_64': {
    url: 'https://github.com/Virakhit/Dump/releases/download/v0.1.1-alpha.1/Dump_0.1.1_x64-setup.exe', signature: 'signed fixture',
  } } };
  const release = { draft: false, tag_name: 'v0.1.1-alpha.1', assets: [
    { name: 'latest.json', browser_download_url: 'https://example.test/latest.json' },
    { id: 123, name: 'Dump_0.1.1_x64-setup.exe', size: 100 }, { name: 'Dump_0.1.1_x64-setup.exe.sig', size: 100 },
  ] };
  let oldVersion, written;
  const github = { rest: { repos: {
    getContent: async () => {
      if (!oldVersion) throw Object.assign(new Error('missing'), { status: 404 });
      return { data: { sha: 'old-sha', content: Buffer.from(JSON.stringify({ version: oldVersion })).toString('base64') } };
    },
    createOrUpdateFileContents: async data => { written = data; },
  } } };
  const args = { github, context: { repo: { owner: 'Virakhit', repo: 'Dump' }, payload: { release } } };
  global.fetch = async () => ({ ok: true, json: async () => manifest });
  try {
    await publish(args);
    assert.equal(written.branch, 'main');
    assert.equal(JSON.parse(Buffer.from(written.content, 'base64')).version, '0.1.1');
    manifest.platforms['windows-x86_64'].url = 'https://api.github.com/repos/Virakhit/Dump/releases/assets/123';
    manifest.platforms['windows-x86_64-nsis'] = { ...manifest.platforms['windows-x86_64'] };
    await publish(args);
    const announced = JSON.parse(Buffer.from(written.content, 'base64'));
    assert.equal(announced.platforms['windows-x86_64'].url, 'https://github.com/Virakhit/Dump/releases/download/v0.1.1-alpha.1/Dump_0.1.1_x64-setup.exe');
    assert.equal(announced.platforms['windows-x86_64-nsis'].url, announced.platforms['windows-x86_64'].url);
    oldVersion = '0.1.0'; await publish(args); assert.equal(written.sha, 'old-sha');
    for (const version of ['0.1.1', '0.2.0', '1.0.0']) {
      oldVersion = version; written = undefined; await publish(args); assert.equal(written, undefined);
    }
    release.draft = true; await assert.rejects(publish(args), /Publish the release/); release.draft = false;
    manifest.platforms['windows-x86_64'].signature = ''; await assert.rejects(publish(args), /Missing signature/);
    manifest.platforms['windows-x86_64'].signature = 'signed fixture';
    const url = manifest.platforms['windows-x86_64'].url;
    manifest.platforms['windows-x86_64'].url = 'https://example.test/installer.exe';
    await assert.rejects(publish(args), /unexpected Windows installer URL/);
    manifest.platforms['windows-x86_64'].url = url;
    release.tag_name = 'v0.1.2-alpha.1'; await assert.rejects(publish(args), /tag and updater version/);
    release.tag_name = 'v0.1.1-alpha.1';
    release.assets.pop(); await assert.rejects(publish(args), /installer and its signature/);
  } finally { global.fetch = originalFetch; }
});
