import { createSignal, Show } from 'solid-js';
import { api } from '../lib/api';

interface InviteDialogProps {
  sessionId: string;
  open: boolean;
  onClose: () => void;
}

export default function InviteDialog(props: InviteDialogProps) {
  const [role, setRole] = createSignal('operator');
  const [inviteUrl, setInviteUrl] = createSignal('');
  const [creating, setCreating] = createSignal(false);
  const [copied, setCopied] = createSignal(false);

  const handleCreate = async () => {
    setCreating(true);
    try {
      const invite = await api.createInvite(props.sessionId, role());
      const url = `${location.origin}/join/${invite.token}`;
      setInviteUrl(url);
    } catch (e) {
      console.error('Failed to create invite:', e);
    }
    setCreating(false);
  };

  const handleCopy = async () => {
    await navigator.clipboard.writeText(inviteUrl());
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleClose = () => {
    setInviteUrl('');
    setCopied(false);
    props.onClose();
  };

  return (
    <Show when={props.open}>
      <div class="dialog-backdrop" onClick={handleClose}>
        <div class="dialog" onClick={(e) => e.stopPropagation()}>
          <h3>Invite to Session</h3>
          <Show when={!inviteUrl()} fallback={
            <div class="invite-result">
              <label>Invite Link</label>
              <div class="invite-url-row">
                <input type="text" value={inviteUrl()} readonly />
                <button class="primary" onClick={handleCopy}>
                  {copied() ? 'Copied!' : 'Copy'}
                </button>
              </div>
              <p class="hint">Share this link with the person you want to invite.</p>
              <button onClick={handleClose} style={{ 'margin-top': '12px', width: '100%' }}>Done</button>
            </div>
          }>
            <div class="invite-form">
              <label>Role</label>
              <div class="role-options">
                <button
                  class={role() === 'operator' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('operator')}
                >
                  Operator
                  <span class="role-desc">Can type and resize</span>
                </button>
                <button
                  class={role() === 'viewer' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('viewer')}
                >
                  Viewer
                  <span class="role-desc">Can only watch</span>
                </button>
              </div>
              <button class="primary" onClick={handleCreate} disabled={creating()} style={{ width: '100%', 'margin-top': '16px' }}>
                {creating() ? 'Creating...' : 'Create Invite Link'}
              </button>
            </div>
          </Show>
          <style>{`
            .dialog-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.5); display: flex; align-items: center; justify-content: center; z-index: 100; }
            .dialog { background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; padding: 24px; width: 400px; max-width: 90vw; }
            .dialog h3 { font-size: 16px; font-weight: 600; margin-bottom: 16px; }
            .dialog label { display: block; font-size: 12px; font-weight: 600; color: var(--text-secondary); margin-bottom: 8px; }
            .role-options { display: flex; gap: 8px; }
            .role-btn { flex: 1; padding: 12px; text-align: left; border-radius: 8px; display: flex; flex-direction: column; gap: 4px; }
            .role-btn.active { border-color: var(--accent); background: rgba(88,166,255,0.1); }
            .role-desc { font-size: 11px; color: var(--text-secondary); }
            .invite-url-row { display: flex; gap: 8px; }
            .invite-url-row input { flex: 1; }
            .hint { font-size: 12px; color: var(--text-secondary); margin-top: 8px; }
          `}</style>
        </div>
      </div>
    </Show>
  );
}
