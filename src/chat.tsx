import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

export type ChatEntry = {
  message: {
    id: string;
    workspace_id: string;
    sender_peer_id: string;
    sender_name: string;
    body: string;
    sent_at: number;
  };
  delivery: { attempted: number; delivered: number; status: 'sending' | 'complete' | 'interrupted' } | null;
};

type Composer = { draft: string; error: string; pending: boolean };
const emptyComposer: Composer = { draft: '', error: '', pending: false };
export const CHAT_BODY_LIMIT = 4096;
const encoder = new TextEncoder();

export function chatBodyBytes(body: string) { return encoder.encode(body).length; }

export function chatValidationError(body: string) {
  if (!body.trim()) return 'Write a message before sending.';
  if (chatBodyBytes(body) > CHAT_BODY_LIMIT) return 'Your message is too long. Keep it within 4,096 UTF-8 bytes.';
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/u.test(body)) return 'Remove unsupported control characters from your message.';
  return '';
}

// Keep drafts and submission locks above the keyed panel so switching workspaces
// cannot clear another draft or submit the same workspace twice.
export function useChatComposers() {
  const [composers, setComposers] = useState<Record<string, Composer>>({});
  const sending = useRef(new Set<string>());
  function update(workspace: string, change: Partial<Composer>) {
    setComposers(current => ({ ...current, [workspace]: { ...(current[workspace] ?? emptyComposer), ...change } }));
  }
  async function send(workspace: string, body: string) {
    if (sending.current.has(workspace)) return;
    const error = chatValidationError(body);
    if (error) { update(workspace, { error }); return; }
    sending.current.add(workspace);
    update(workspace, { pending: true, error: '' });
    try {
      await invoke<string>('send_chat_message', { workspace, body });
      setComposers(current => {
        const previous = current[workspace] ?? emptyComposer;
        return { ...current, [workspace]: { ...previous, draft: previous.draft === body ? '' : previous.draft, error: '' } };
      });
    } catch (error) {
      update(workspace, { error: String(error) });
    } finally {
      sending.current.delete(workspace);
      update(workspace, { pending: false });
    }
  }
  return {
    composer: (workspace: string) => composers[workspace] ?? emptyComposer,
    setDraft: (workspace: string, draft: string) => update(workspace, { draft, error: '' }),
    send,
  };
}

export function chatDeliveryLabel(delivery: NonNullable<ChatEntry['delivery']>) {
  const { attempted, delivered, status } = delivery;
  if (status === 'sending') return attempted ? `Sending · ${delivered} of ${attempted} devices received` : 'Sending…';
  if (status === 'interrupted') return `Delivery interrupted · ${delivered} of ${attempted} devices received`;
  if (!attempted) return 'Saved on this device · No other devices online';
  if (delivered < attempted) return `${delivered} of ${attempted} devices received · Some devices did not confirm`;
  return `Received by ${delivered} ${delivered === 1 ? 'device' : 'devices'}`;
}

function MessageTime({ seconds }: { seconds: number }) {
  const date = new Date(seconds * 1000);
  if (!Number.isFinite(date.getTime())) return <span className="chat-time">Time unavailable</span>;
  return <time className="chat-time" dateTime={date.toISOString()} title={date.toLocaleString()}>{date.toLocaleString(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</time>;
}

export function ChatPanel({ workspace, workspaceName, peerId, messages, draft, error, pending, canSend, onDraftChange, onSend }: {
  workspace: string;
  workspaceName: string;
  peerId: string;
  messages: ChatEntry[];
  draft: string;
  error: string;
  pending: boolean;
  canSend: boolean;
  onDraftChange: (draft: string) => void;
  onSend: (body: string) => void;
}) {
  const history = useRef<HTMLDivElement>(null);
  const nearBottom = useRef(true);
  const entries = messages.filter(entry => entry.message.workspace_id === workspace);
  const latest = entries.at(-1)?.message;
  const bytes = chatBodyBytes(draft);
  const oversize = bytes > CHAT_BODY_LIMIT;
  useEffect(() => {
    if (nearBottom.current || latest?.sender_peer_id === peerId) {
      const element = history.current;
      if (element) element.scrollTop = element.scrollHeight;
    }
  }, [latest?.id, peerId]);
  function submit() { if (canSend && !pending) onSend(draft); }

  return <div className="chat-panel">
    <div className="chat-heading"><h2>Group chat</h2><span>Local history</span></div>
    <p className="chat-scope">New messages go to online members in this workspace. Offline members do not receive earlier messages.</p>
    <div className="chat-history" ref={history} role="log" aria-label={`${workspaceName} messages`} aria-live="polite" aria-relevant="additions" tabIndex={0} onScroll={event => {
      const element = event.currentTarget;
      nearBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
    }}>
      {!entries.length && <div className="chat-empty"><span aria-hidden="true">···</span><strong>Start the conversation</strong><p>Say hello or talk about what you’re sharing.<br />Messages you send and receive are saved on this device.</p></div>}
      {entries.map(({ message, delivery }) => {
        const own = message.sender_peer_id === peerId;
        const warning = delivery && (delivery.status === 'interrupted' || delivery.status === 'complete' && delivery.delivered < delivery.attempted);
        return <article className={`chat-message ${own ? 'chat-own' : ''}`} key={`${message.sender_peer_id}:${message.id}`}>
          <div className="chat-message-meta"><strong title={message.sender_peer_id}>{message.sender_name || 'Unnamed device'}{own && <span> (you)</span>}</strong><MessageTime seconds={message.sent_at} /></div>
          <p className="chat-body">{message.body}</p>
          {delivery && <p className={`chat-delivery ${warning ? 'chat-delivery-warning' : ''}`} title="Receipt confirms delivery to a device, not that someone has read the message.">{delivery.status === 'sending' && <span className="spinner" aria-hidden="true" />}{chatDeliveryLabel(delivery)}</p>}
        </article>;
      })}
    </div>
    <form className="chat-composer" onSubmit={event => { event.preventDefault(); submit(); }}>
      <label htmlFor="chat-message">Message {workspaceName}</label>
      <textarea id="chat-message" rows={3} placeholder={canSend ? 'Write a message…' : 'You must be a member to send messages.'} value={draft} disabled={!canSend || pending} aria-describedby={`chat-compose-hint chat-byte-count${error ? ' chat-compose-error' : ''}`} aria-invalid={oversize || !!error} onChange={event => onDraftChange(event.target.value)} onKeyDown={event => {
        if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
          event.preventDefault(); submit();
        }
      }} />
      <div className="chat-composer-actions"><div><p id="chat-compose-hint">Enter to send · Shift+Enter for a new line</p><span id="chat-byte-count" className={oversize ? 'danger' : ''}>{bytes.toLocaleString()} / 4,096 bytes</span></div><button className="primary" type="submit" disabled={!canSend || pending}>{pending ? 'Saving…' : 'Send'}<span aria-hidden="true">↑</span></button></div>
      {error && <p className="chat-compose-error" id="chat-compose-error" role="alert">{error}</p>}
    </form>
  </div>;
}
