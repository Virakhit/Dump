import React from 'react';

export type ChatEntry = {
  message: { id: string; workspace_id: string; sender_peer_id: string; body: string; sent_at: number };
  sender_name: string;
  delivery: { status: 'sending' | 'complete' | 'interrupted'; attempted: number; acknowledged: number; offline: number } | null;
};
function receipt(entry: ChatEntry) {
  const d = entry.delivery;
  if (!d) return 'Stored on this device';
  if (d.status === 'sending') return `Saved locally · sending to ${d.attempted} devices`;
  const confirmations = `${d.acknowledged} of ${d.attempted} contacted devices confirmed storage`;
  if (d.status === 'interrupted') return `Saved locally · interrupted · ${confirmations}. Not queued.`;
  if (!d.attempted) return 'Saved locally · No other devices online. Not queued.';
  return `Saved locally · ${confirmations}${d.offline ? ` · ${d.offline} offline; not queued` : ''}`;
}
export function ChatPanel({ workspace, entries, draft, sending, error, canSend, onDraft, onSend }: {
  workspace: string; entries: ChatEntry[]; draft: string; sending: boolean; error: string;
  canSend: boolean; onDraft: (draft: string) => void; onSend: () => void;
}) {
  const bytes = new TextEncoder().encode(draft).length;
  const shown = entries.filter(entry => entry.message.workspace_id === workspace);
  return <section className="workspace-chat" aria-label="Workspace chat">
    <div className="list-heading"><h2>Workspace chat</h2></div>
    <p className="hint">Messages stay in local history. Only online approved devices are contacted; offline devices are not queued. A storage receipt does not mean a person read the message.</p>
    <div className="chat-history" role="log" aria-live="polite" aria-label="Messages">
      {shown.map(entry => <article className="chat-entry" key={`${entry.message.sender_peer_id}:${entry.message.id}`}>
        <strong>{entry.sender_name}</strong><p className="chat-body">{entry.message.body}</p>
        <small>{receipt(entry)}</small>
      </article>)}
      {!shown.length && <p className="hint">No messages in this workspace yet.</p>}
    </div>
    <form onSubmit={event => { event.preventDefault(); if (canSend && !sending && draft.trim() && bytes <= 4096) onSend(); }}>
      <label>Message<textarea aria-label="Message" rows={3} maxLength={4096} value={draft} onChange={event => onDraft(event.target.value)} disabled={!canSend} /></label>
      <div className="chat-compose-actions"><small>{bytes} / 4096 UTF-8 bytes</small><button className="primary" type="submit" disabled={!canSend || sending || !draft.trim() || bytes > 4096}>{sending ? 'Saving locally…' : 'Send message'}</button></div>
      {!canSend && <p className="hint">Choose a workspace you belong to before sending.</p>}
      {error && <p role="alert" className="danger">{error}</p>}
    </form>
  </section>;
}
