import React, { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import './style.css';

type Member = { peer_id: string; name: string };
type Snapshot = { workspace_id: string; name: string; owner_peer_id: string; members: Member[]; revision: number };
type File = { manifest: { file_id: string; name: string; size: number; sha256: string; owner_peer_id: string }; mine: boolean; owner_name: string };
type Transfer = { id: string; name: string; direction: string; peer_id: string; bytes: number; total: number; status: string; error: string | null };
type View = { peer_id: string; fingerprint: string; device_name: string; active: string | null; workspaces: { snapshot: Snapshot; mine: boolean }[]; files: File[]; online: string[]; pending: { peer_id: string; name: string; fingerprint: string }[]; preparing: string[]; joining: boolean; network_status: string; transfers: Transfer[]; invite_url: string | null; last_error: string | null };
type ModalKind = 'create' | 'join' | 'name' | 'identity' | 'invite' | 'updates' | null;
const desktop = isTauri();
const empty: View = { peer_id: '', fingerprint: '', device_name: 'My device', active: null, workspaces: [], files: [], online: [], pending: [], preparing: [], joining: false, network_status: desktop ? 'Starting…' : 'Desktop app required', transfers: [], invite_url: null, last_error: null };
function size(bytes: number) { if (bytes < 1024) return `${bytes} B`; const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), 4); return `${(bytes / 1024 ** unit).toFixed(unit > 1 ? 1 : 0)} ${['B', 'KB', 'MB', 'GB', 'TB'][unit]}`; }
function short(id: string) { return id ? `${id.slice(0, 8)}…${id.slice(-6)}` : 'Starting…'; }
function Modal({ title, children, close, closeDisabled = false }: { title: string; children: React.ReactNode; close: () => void; closeDisabled?: boolean }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { ref.current?.showModal(); }, []);
  return <dialog ref={ref} onCancel={e => { e.preventDefault(); if (!closeDisabled) close(); }} aria-labelledby="dialog-title"><div className="dialog-head"><h2 id="dialog-title">{title}</h2><button className="icon-button" aria-label="Close dialog" disabled={closeDisabled} onClick={close}>×</button></div>{children}</dialog>;
}

function App() {
  const [view, setView] = useState(empty);
  const [modal, setModal] = useState<ModalKind>(null);
  const [value, setValue] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [tab, setTab] = useState<'files' | 'members'>('files');
  const [verified, setVerified] = useState<Record<string, boolean>>({});
  const [remove, setRemove] = useState<Member | null>(null);
  const [switchTo, setSwitchTo] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [version, setVersion] = useState('');
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updateStatus, setUpdateStatus] = useState<'idle' | 'checking' | 'current' | 'available' | 'downloading' | 'installing' | 'error'>('idle');
  const [updateError, setUpdateError] = useState('');
  const [updateProgress, setUpdateProgress] = useState<{ downloaded: number; total: number | null }>({ downloaded: 0, total: null });
  const dropZone = useRef<HTMLButtonElement>(null);
  const active = view.workspaces.find(w => w.snapshot.workspace_id === view.active);
  const mine = active?.mine ?? false;
  const allowed = active?.snapshot.members.some(m => m.peer_id === view.peer_id) ?? false;
  const unfinished = !!view.preparing.length || view.transfers.some(t => !['Completed', 'Cancelled', 'Failed'].includes(t.status));
  const updating = updateStatus === 'downloading' || updateStatus === 'installing';
  useEffect(() => {
    if (!desktop) return;
    let disposed = false; let unlisten: (() => void) | undefined;
    void listen<View>('dump-state', e => { if (!disposed) setView(e.payload); }).then(stop => { if (disposed) stop(); else unlisten = stop; });
    void invoke<View>('get_state').then(setView).catch(e => setError(String(e)));
    void getVersion().then(setVersion).catch(e => setError(String(e)));
    return () => { disposed = true; unlisten?.(); };
  }, []);
  useEffect(() => {
    if (!desktop) return;
    let disposed = false; let unlisten: (() => void) | undefined;
    void getCurrentWebviewWindow().onDragDropEvent(e => {
      if (disposed || e.payload.type !== 'drop' || !allowed || busy || modal || remove || switchTo || tab !== 'files') return;
      const rect = dropZone.current?.getBoundingClientRect();
      const x = e.payload.position.x / window.devicePixelRatio, y = e.payload.position.y / window.devicePixelRatio;
      if (!rect || x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return;
      void call('share_dropped_files', { paths: e.payload.paths, workspace: view.active });
    }).then(stop => { if (disposed) stop(); else unlisten = stop; }).catch(e => setError(String(e)));
    return () => { disposed = true; unlisten?.(); };
  }, [view.active, allowed, busy, modal, remove, switchTo, tab]);
  async function call(command: string, args?: Record<string, unknown>) {
    setBusy(true); setError('');
    try { await invoke(command, args); return true; } catch (e) { setError(String(e)); return false; } finally { setBusy(false); }
  }
  const act = (action: Record<string, unknown>) => call('dispatch', { action });
  function open(kind: ModalKind) { setValue(kind === 'name' ? view.device_name : ''); setCopied(false); setModal(kind); }
  async function copy(text: string) { try { await navigator.clipboard.writeText(text); setCopied(true); } catch { setError('Could not copy. Select and copy the text manually.'); } }
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const action = modal === 'create' ? { type: 'create_workspace', name: value.trim() } : modal === 'name' ? { type: 'set_name', name: value.trim() } : { type: 'join', invite: value.trim() };
    if (await act(action)) setModal(null);
  }
  async function invite() { if (await act({ type: 'create_invite' })) open('invite'); }
  async function checkUpdate() {
    setModal('updates'); setUpdateStatus('checking'); setUpdateError(''); setUpdateVersion(null);
    try {
      const next = await invoke<string | null>('check_update');
      setUpdateVersion(next); setUpdateStatus(next ? 'available' : 'current');
    } catch (e) { setUpdateError(String(e)); setUpdateStatus('error'); }
  }
  async function installUpdate() {
    if (!updateVersion || unfinished || busy || updating) return;
    setBusy(true); setUpdateError(''); setUpdateStatus('downloading'); setUpdateProgress({ downloaded: 0, total: null });
    const onProgress = new Channel<{ downloaded: number; total: number | null }>();
    onProgress.onmessage = progress => { setUpdateProgress(progress); if (progress.total && progress.downloaded >= progress.total) setUpdateStatus('installing'); };
    try { await invoke('install_update', { version: updateVersion, onProgress }); setUpdateStatus('installing'); }
    catch (e) { setUpdateError(String(e)); setUpdateStatus('available'); }
    finally { setBusy(false); }
  }
  async function activate(id: string) { if (await act({ type: 'activate', id })) { setTab('files'); setSwitchTo(null); } }
  function selectWorkspace(id: string) {
    if (id === view.active) return;
    if (view.transfers.some(t => !['Completed', 'Cancelled', 'Failed'].includes(t.status))) setSwitchTo(id);
    else void activate(id);
  }
  const files = view.files.filter(f => f.manifest.name.toLowerCase().includes(query.toLowerCase()));
  const shownError = error || view.last_error;

  return <div className="app">
    <aside className="sidebar">
      <a className="brand" href="#" aria-label="Dump home"><span className="brand-mark">d</span>dump<span className="beta">ALPHA</span></a>
      <div className="sidebar-label">YOUR WORKSPACES<button className="icon-button" disabled={!desktop || busy} onClick={() => open('create')} aria-label="Create workspace">+</button></div>
      <nav aria-label="Workspaces">{view.workspaces.map(w => <button key={w.snapshot.workspace_id} className={`workspace-button ${view.active === w.snapshot.workspace_id ? 'selected' : ''}`} disabled={busy} onClick={() => selectWorkspace(w.snapshot.workspace_id)}><span className="workspace-icon">{w.snapshot.name.slice(0, 1).toUpperCase()}</span><span>{w.snapshot.name}</span>{w.mine && <span className="owner-dot" title="You own this workspace" />}</button>)}{!view.workspaces.length && <p className="sidebar-empty">Your groups will appear here.</p>}</nav>
      <button className="join-button" disabled={!desktop || busy} onClick={() => open('join')}><span>↗</span> Join with an invite</button>
      <div className="sidebar-bottom"><div className="local-note"><span className="status-dot" /> DIRECT. PRIVATE. LOCAL.<p>Files stay on your device.<br />Only the selected workspace shares files.</p></div><button className="update-button" disabled={!desktop || busy || updating || updateStatus === 'checking'} onClick={() => void checkUpdate()}>Check for updates <small>{version && `v${version}`}</small></button><button className="device" onClick={() => open('identity')} disabled={!desktop || updating}><span className="avatar">{view.device_name.slice(0, 1).toUpperCase()}</span><span><strong>{view.device_name}</strong><small>{short(view.peer_id)}</small></span><span>···</span></button></div>
    </aside>
    <main>
      <header><div><div className="eyebrow">YOUR LOCAL NETWORK</div><h1>{active?.snapshot.name ?? 'A little less cloud.'}</h1><p>{active ? `${active.snapshot.members.length} ${active.snapshot.members.length === 1 ? 'device' : 'devices'} · ${view.online.length + (allowed ? 1 : 0)} online` : 'A simple place to share files, directly.'}</p></div><div className="header-actions"><span className="network-badge"><span className="status-dot" /> LAN only</span>{mine && <button disabled={busy} onClick={() => void invite()}>Invite a friend <span>↗</span></button>}</div></header>
      {!desktop && <div className="notice">This is the interface preview. Open the Dump desktop app to create groups and transfer files.</div>}
      {shownError && <div className="error" role="alert"><span>{shownError}</span><button aria-label="Dismiss error" onClick={() => { setError(''); if (view.last_error) void act({ type: 'dismiss_error' }); }}>×</button></div>}
      {view.joining && <div className="notice"><div><strong>Waiting for the workspace owner</strong><p>Keep both devices on the same LAN. The owner needs to be online and approve your identity. Approval selects that workspace and cancels unfinished transfers.</p></div><button onClick={() => void act({ type: 'cancel_join' })}>Cancel</button></div>}
      {active && !allowed && <div className="error" role="alert">You are no longer a member of this workspace. Sharing and receiving are disabled.</div>}
      {!active ? <section className="welcome"><div className="welcome-art" aria-hidden="true"><div className="paper">↗<span>file.zip</span></div><div className="link-line" /><div className="paper second">✓<span>file.zip</span></div></div><div className="eyebrow">FROM YOUR DEVICE TO THEIRS</div><h2>Good files. Good company.</h2><p>Make a private group, invite someone you trust,<br />and share files without sending them to the cloud.</p><div className="welcome-actions"><button className="primary" disabled={!desktop || busy} onClick={() => open('create')}>Create a workspace <span>+</span></button><button disabled={!desktop || busy} onClick={() => open('join')}>I have an invite</button></div><div className="welcome-foot">No account. No server. Just your local network.</div></section> : <>
        <div className="tabs" role="tablist" aria-label="Workspace content"><button id="files-tab" role="tab" aria-selected={tab === 'files'} aria-controls="workspace-panel" className={tab === 'files' ? 'active' : ''} onClick={() => setTab('files')}>Shared files <span>{view.files.length}</span></button><button id="members-tab" role="tab" aria-selected={tab === 'members'} aria-controls="workspace-panel" className={tab === 'members' ? 'active' : ''} onClick={() => setTab('members')}>Members <span>{active.snapshot.members.length}</span></button></div>
        <section id="workspace-panel" role="tabpanel" aria-labelledby={tab === 'files' ? 'files-tab' : 'members-tab'}>
          {tab === 'files' ? <><button ref={dropZone} className="drop-zone" disabled={!allowed || busy} onClick={() => void call('choose_share_files')}><span className="drop-icon">↥</span><strong>Drop files here to share</strong><span>or click to choose files</span><small>Original files stay on your device. Available while you’re online.</small></button>{view.preparing.map((name, i) => <div className="preparing" key={`${name}-${i}`} role="status"><span className="spinner" /> Preparing {name}…</div>)}<div className="list-heading"><h2>Available now</h2><label className="search"><span aria-hidden="true">⌕</span><input aria-label="Search shared files" placeholder="Find a file…" value={query} onChange={e => setQuery(e.target.value)} /></label></div>
            <div className="file-list">{files.map(file => <article className="file-row" key={file.manifest.file_id}><span className="file-icon" aria-hidden="true">▤</span><div className="file-info"><strong title={file.manifest.name}>{file.manifest.name}</strong><span>{size(file.manifest.size)} <span className="separator">·</span> {file.mine ? 'Your device' : file.owner_name}</span></div><span className="availability"><span className="status-dot" /> Available</span>{file.mine ? <button className="quiet" disabled={busy} onClick={() => void act({ type: 'unshare', id: file.manifest.file_id })}>Stop sharing</button> : <button disabled={!allowed || busy} onClick={() => void call('receive_file', { fileId: file.manifest.file_id })}>Receive <span>↓</span></button>}</article>)}{!files.length && <div className="empty-list"><strong>{query ? 'No matching files' : 'Nothing shared yet'}</strong><p>{query ? 'Try a different name.' : 'Share a file, or wait for a friend to come online.'}</p></div>}</div></> : <><div className="list-heading"><h2>People you trust, devices you approve</h2></div>{view.pending.map(p => <article className="request-card" key={p.peer_id}><strong>{p.name} wants to join</strong><p>Compare this fingerprint with the requester through a trusted conversation. A name alone does not verify identity.</p><code>{p.fingerprint}</code><label className="check"><input type="checkbox" checked={verified[p.peer_id] ?? false} onChange={e => setVerified({ ...verified, [p.peer_id]: e.target.checked })} /> I compared this device’s fingerprint.</label><div><button className="primary" disabled={busy || !verified[p.peer_id]} onClick={() => void act({ type: 'approve', peer_id: p.peer_id })}>Approve device</button><button disabled={busy} onClick={() => void act({ type: 'reject', peer_id: p.peer_id })}>Reject</button></div></article>)}{active.snapshot.members.map(member => <article className="member-row" key={member.peer_id}><span className="avatar">{member.name.slice(0, 1).toUpperCase()}</span><div className="file-info"><strong>{member.peer_id === view.peer_id ? view.device_name : member.name} {member.peer_id === view.peer_id && <small>(you)</small>}</strong><span>{short(member.peer_id)} {member.peer_id === active.snapshot.owner_peer_id && '· Workspace owner'}</span></div><span className="availability"><span className={`status-dot ${member.peer_id === view.peer_id || view.online.includes(member.peer_id) ? '' : 'offline'}`} />{member.peer_id === view.peer_id || view.online.includes(member.peer_id) ? 'Online' : 'Offline'}</span>{mine && member.peer_id !== view.peer_id && <button className="quiet danger" onClick={() => setRemove(member)}>Remove</button>}</article>)}<p className="hint">Each device has its own identity. The owner must be online to approve a new device.</p></>}
        </section>
        {!!view.pending.length && tab !== 'members' && <button className="pending-banner" onClick={() => setTab('members')}>{view.pending.length} join {view.pending.length === 1 ? 'request' : 'requests'} waiting for approval <span>→</span></button>}
      </>}
      {!!view.transfers.length && <section className="transfer-section"><h2>Transfers</h2>{view.transfers.map(t => <article className="transfer" key={t.id}><div className="transfer-title"><strong>{t.name}</strong><span>{t.direction} · {t.status}</span></div><progress value={t.status === 'Completed' ? 1 : t.total ? t.bytes / t.total : 0} max={1} aria-label={`${t.name} transfer progress`} /><div className="transfer-meta"><span>{size(t.bytes)} / {size(t.total)}{t.error && ` · ${t.error}`}</span>{!['Completed', 'Cancelled', 'Failed'].includes(t.status) && <button className="quiet" onClick={() => void act({ type: 'cancel_transfer', id: t.id })}>Cancel</button>}</div></article>)}</section>}
      <footer><span><span className="status-dot" /> {view.network_status}</span><span>End-to-end encrypted · Files verified with SHA-256</span></footer>
    </main>
    {modal === 'updates' && <Modal title="App updates" closeDisabled={updating} close={() => setModal(null)}>
      <p>Installed version: <strong>{version || 'Loading…'}</strong>. Checks and downloads use GitHub over the internet; your shared files stay on your local network.</p>
      <div role="status" aria-live="polite">
        {updateStatus === 'checking' && <p>Checking for updates…</p>}
        {updateStatus === 'current' && <p>You have the latest published version.</p>}
        {updateVersion && <p>Version <strong>{updateVersion}</strong> is available.</p>}
        {updating && <><p>{updateStatus === 'installing' ? 'Verifying the download and starting the installer…' : 'Downloading update…'}</p><progress value={updateProgress.total ? updateProgress.downloaded / updateProgress.total : undefined} max={1} aria-label="Update download progress" /><p>{size(updateProgress.downloaded)}{updateProgress.total ? ` / ${size(updateProgress.total)}` : ''}</p></>}
      </div>
      {updateError && <p className="danger" role="alert">{updateError}</p>}
      {updateStatus === 'available' && <><p>Dump will close and restart to install the update. Your device identity and workspaces are kept.</p>{unfinished && <p>Finish or cancel transfers and file preparation before updating.</p>}<button className="primary" disabled={busy || unfinished} onClick={() => void installUpdate()}>Update to {updateVersion}</button></>}
      {!updating && updateStatus !== 'checking' && <button className="quiet" onClick={() => void checkUpdate()}>Check again</button>}
    </Modal>}
    {modal && modal !== 'updates' && <Modal title={{ create: 'Create a workspace', join: 'Join a workspace', name: 'Name your device', identity: 'Your device identity', invite: 'Invite someone you trust' }[modal]} close={() => setModal(null)}>{['create', 'join', 'name'].includes(modal) ? <form onSubmit={e => void submit(e)}><p>{modal === 'join' ? 'Paste the invite your workspace owner sent you. Approval selects that workspace and cancels unfinished transfers.' : modal === 'create' ? 'A private group for sharing on your local network. Creating it selects this workspace and cancels unfinished transfers.' : 'This name helps friends recognize your device. Your cryptographic identity stays the same.'}</p><label>{modal === 'join' ? 'Invitation' : modal === 'create' ? 'Workspace name' : 'Device name'}<input autoFocus required maxLength={modal === 'join' ? 2048 : 100} value={value} onChange={e => setValue(e.target.value)} placeholder={modal === 'join' ? 'dump://join/…' : modal === 'create' ? 'Weekend projects' : 'My laptop'} /></label><button className="primary" disabled={busy || !value.trim()} type="submit">{modal === 'join' ? 'Request to join' : modal === 'create' ? 'Create workspace' : 'Save name'}</button></form> : modal === 'invite' ? <><p>This invite expires in 24 hours and approves one device. You still need to compare its fingerprint and accept the request.</p><label>Invitation<textarea readOnly value={view.invite_url ?? ''} rows={4} /></label><button className="primary" onClick={() => void copy(view.invite_url ?? '')}>{copied ? 'Copied' : 'Copy invitation'}</button><p className="hint">Both devices must be on the same LAN. Keep Dump open while your friend joins.</p></> : <><p>Send this fingerprint to your workspace owner through a trusted conversation before they approve your device.</p><label>Fingerprint<code className="fingerprint">{view.fingerprint}</code></label><button onClick={() => void copy(view.fingerprint)}>{copied ? 'Copied' : 'Copy fingerprint'}</button><label>Peer ID<code className="fingerprint">{view.peer_id}</code></label><button className="quiet" onClick={() => open('name')}>Edit device name</button><p className="hint">LAN discovery exposes your device address and Peer ID, but does not announce your group or filenames.</p></>}{error && <p className="danger" role="alert">{error}</p>}</Modal>}
    {remove && <Modal title={`Remove ${remove.name}?`} close={() => setRemove(null)}><p>Each device blocks this member after receiving the updated membership. Offline or isolated devices may still grant access until then.</p><p>Files they already received cannot be recalled.</p><div className="dialog-actions"><button onClick={() => setRemove(null)}>Keep member</button><button className="destructive" disabled={busy} onClick={async () => { if (await act({ type: 'remove_member', peer_id: remove.peer_id })) setRemove(null); }}>Remove member</button></div></Modal>}
    {switchTo && <Modal title="Switch workspace?" close={() => setSwitchTo(null)}><p>Switching cancels your unfinished transfers. Only the selected workspace shares files.</p><div className="dialog-actions"><button onClick={() => setSwitchTo(null)}>Stay here</button><button disabled={busy} onClick={() => void activate(switchTo)}>Switch workspace</button></div></Modal>}
  </div>;
}
createRoot(document.getElementById('root')!).render(<App />);
