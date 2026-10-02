// Publish a channel manifest only after its signed installer is public.
// This includes alpha releases, which GitHub's /releases/latest endpoint skips.
module.exports = async function publishUpdate({ github, context }) {
  const { owner, repo } = context.repo;
  const release = context.payload.release;
  if (release.draft) throw new Error('Publish the release before announcing an update.');
  const asset = release.assets.find(a => a.name === 'latest.json');
  if (!asset) throw new Error('Release is missing the signed updater latest.json asset.');
  const response = await fetch(asset.browser_download_url);
  if (!response.ok) throw new Error(`Manifest download failed: ${response.status}`);
  const manifest = await response.json();
  const parts = versionParts(manifest.version);
  const platform = manifest.platforms?.['windows-x86_64'];
  const tag = release.tag_name;
  if (!(tag === `v${manifest.version}` || new RegExp(`^v${manifest.version.replaceAll('.', '\\.')}\\-alpha\\.[0-9]+$`).test(tag))) {
    throw new Error('Release tag and updater version differ.');
  }
  const expectedUrl = `https://github.com/${owner}/${repo}/releases/download/${tag}/Dump_${manifest.version}_x64-setup.exe`;
  const installer = release.assets.find(a => a.name === `Dump_${manifest.version}_x64-setup.exe`);
  const signature = release.assets.find(a => a.name === `${installer?.name}.sig`);
  if (!installer || !signature || installer.size <= 0 || signature.size <= 0) {
    throw new Error('Publish both the installer and its signature.');
  }
  const assetApiUrl = `https://api.github.com/repos/${owner}/${repo}/releases/assets/${installer.id}`;
  if (!platform) throw new Error('Missing Windows update platform.');
  for (const key of ['windows-x86_64', 'windows-x86_64-nsis']) {
    const entry = manifest.platforms[key];
    if (!entry) continue;
    if (![expectedUrl, assetApiUrl].includes(entry.url) || typeof entry.signature !== 'string' || !entry.signature.trim()) {
      throw new Error('Missing signature or unexpected Windows installer URL.');
    }
    // Tauri Action v1 can emit GitHub asset API URLs; publish public download URLs for both aliases.
    entry.url = expectedUrl;
  }
  let previous;
  try {
    previous = (await github.rest.repos.getContent({ owner, repo, path: 'releases/latest.json', ref: 'main' })).data;
  } catch (error) { if (error.status !== 404) throw error; }
  if (previous) {
    const old = JSON.parse(Buffer.from(previous.content, 'base64').toString('utf8'));
    const oldParts = versionParts(old.version);
    const difference = parts.map((n, i) => n - oldParts[i]).find(n => n !== 0) ?? 0;
    if (difference <= 0) return; // Re-publishing an older/equal release cannot roll back the channel.
  }
  await github.rest.repos.createOrUpdateFileContents({
    owner, repo, branch: 'main', path: 'releases/latest.json',
    message: `Announce Dump ${manifest.version} update`,
    content: Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`).toString('base64'),
    ...(previous ? { sha: previous.sha } : {}),
  });
};

function versionParts(version) {
  if (typeof version !== 'string' || !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(version)) {
    throw new Error('Application versions must use major.minor.patch.');
  }
  const parts = version.split('.').map(Number);
  if (!parts.every(Number.isSafeInteger)) throw new Error('Version number is too large.');
  return parts;
}
