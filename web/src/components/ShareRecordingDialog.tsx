// web/src/components/ShareRecordingDialog.tsx
import { createSignal, onMount, For, Show } from 'solid-js';
import type { JSX } from 'solid-js';
import { api, errorMessage } from '../lib/api';
import { formatDate } from '../lib/format';
import type { RecordingShare } from '../lib/protocol';

const SHARE_DATE_OPTIONS: Intl.DateTimeFormatOptions = {
  month: 'short',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
};

function formatUses(share: RecordingShare): string {
  if (share.max_uses === 0) {
    return `Uses: ${share.used_count} / unlimited`;
  }
  return `Uses: ${share.used_count} / ${share.max_uses}`;
}

export interface ShareRecordingDialogProps {
  recordingId: string;
  onClose: () => void;
}

export default function ShareRecordingDialog(props: ShareRecordingDialogProps): JSX.Element {
  const [shares, setShares] = createSignal<RecordingShare[]>([]);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal('');
  const [creating, setCreating] = createSignal(false);
  const [newToken, setNewToken] = createSignal('');
  const [copyLabel, setCopyLabel] = createSignal('Copy');
  const [revokingSet, setRevokingSet] = createSignal<Set<string>>(new Set());

  onMount(async () => {
    await loadShares();
  });

  async function loadShares() {
    setLoading(true);
    setError('');
    try {
      const result = await api.listRecordingShares(props.recordingId);
      setShares(result);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  async function handleCreate() {
    setCreating(true);
    setError('');
    setNewToken('');
    try {
      const share = await api.createRecordingShare(props.recordingId);
      // Construct the shareable URL using a URL fragment (#token=…)
      // instead of a query string. Fragments never hit the server, so
      // reverse-proxy and gateway access logs can't capture the raw
      // share secret — a token leaked through `nginx_access.log` used
      // to grant replay for the full TTL until the owner revoked it.
      // The player reads `location.hash`, strips it via
      // `history.replaceState`, and sends the token over the
      // `X-Share-Token` header on its one `/data` fetch.
      const url = `${window.location.origin}/recordings/${props.recordingId}/play#token=${encodeURIComponent(share.token)}`;
      setNewToken(url);
      // Reload the list so the new entry appears
      await loadShares();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setCreating(false);
    }
  }

  async function handleRevoke(tokenSha256: string) {
    setRevokingSet((prev) => new Set([...prev, tokenSha256]));
    setError('');
    try {
      await api.deleteRecordingShare(props.recordingId, tokenSha256);
      setShares((prev) => prev.filter((s) => s.token_sha256 !== tokenSha256));
      // Clear newToken if user revokes the just-created share (sha matches prefix)
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRevokingSet((prev) => {
        const next = new Set(prev);
        next.delete(tokenSha256);
        return next;
      });
    }
  }

  function handleCopy() {
    const token = newToken();
    if (!token) return;
    navigator.clipboard.writeText(token).then(() => {
      setCopyLabel('Copied!');
      setTimeout(() => setCopyLabel('Copy'), 2000);
    }).catch(() => {
      setCopyLabel('Copy');
    });
  }

  return (
    <div class="srd-overlay" onClick={(e) => { if (e.target === e.currentTarget) props.onClose(); }}>
      <div class="srd-modal" role="dialog" aria-modal="true" aria-label="Share recording">
        <div class="srd-header">
          <h2 class="srd-title">Share Recording</h2>
          <button type="button" class="srd-close" aria-label="Close" onClick={props.onClose}>
            ✕
          </button>
        </div>

        <div class="srd-body">
          <Show when={error()}>
            <p class="srd-error">{error()}</p>
          </Show>

          {/* Create new share link */}
          <div class="srd-create-row">
            <button
              type="button"
              class="srd-create-btn"
              onClick={handleCreate}
              disabled={creating()}
            >
              {creating() ? 'Creating…' : '+ Create Share Link'}
            </button>
          </div>

          {/* Newly created token URL */}
          <Show when={newToken()}>
            <div class="srd-token-box">
              <p class="srd-token-hint">Share this link (token shown only once):</p>
              <div class="srd-token-row">
                <input
                  class="srd-token-input"
                  type="text"
                  readOnly
                  value={newToken()}
                  onFocus={(e) => e.currentTarget.select()}
                  aria-label="Share link"
                />
                <button type="button" class="srd-copy-btn" onClick={handleCopy}>
                  {copyLabel()}
                </button>
              </div>
            </div>
          </Show>

          {/* Existing shares */}
          <div class="srd-shares-section">
            <h3 class="srd-section-title">Existing Links</h3>

            <Show when={loading()}>
              <p class="srd-muted">Loading…</p>
            </Show>

            <Show when={!loading() && shares().length === 0}>
              <p class="srd-muted">No share links yet.</p>
            </Show>

            <Show when={!loading() && shares().length > 0}>
              <ul class="srd-share-list">
                <For each={shares()}>
                  {(share) => (
                    <li class="srd-share-item">
                      <div class="srd-share-info">
                        <span class="srd-share-prefix mono">…{share.token_sha256.slice(-8)}</span>
                        <span class="srd-share-meta">
                          {formatUses(share)}
                        </span>
                        <Show when={share.expires_at}>
                          <span class="srd-share-expiry">
                            Expires {formatDate(share.expires_at!, SHARE_DATE_OPTIONS)}
                          </span>
                        </Show>
                        <span class="srd-share-created">
                          Created {formatDate(share.created_at, SHARE_DATE_OPTIONS)}
                        </span>
                      </div>
                      <button
                        type="button"
                        class="srd-revoke-btn"
                        disabled={revokingSet().has(share.token_sha256)}
                        onClick={() => handleRevoke(share.token_sha256)}
                      >
                        {revokingSet().has(share.token_sha256) ? 'Revoking…' : 'Revoke'}
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </div>
        </div>
      </div>

      <style>{`
        .srd-overlay {
          position: fixed;
          inset: 0;
          background: rgba(0, 0, 0, 0.6);
          z-index: 50;
          display: flex;
          align-items: center;
          justify-content: center;
          padding: 16px;
        }

        .srd-modal {
          background: var(--bg-secondary, #161b22);
          border: 1px solid var(--border, #30363d);
          border-radius: 10px;
          width: 100%;
          max-width: 520px;
          max-height: 80vh;
          display: flex;
          flex-direction: column;
          box-shadow: 0 16px 40px rgba(0, 0, 0, 0.4);
        }

        .srd-header {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 16px 20px;
          border-bottom: 1px solid var(--border, #30363d);
        }

        .srd-title {
          font-size: 15px;
          font-weight: 600;
          margin: 0;
        }

        .srd-close {
          background: transparent;
          border: none;
          color: var(--text-secondary, #8b949e);
          font-size: 16px;
          cursor: pointer;
          padding: 4px;
          border-radius: 4px;
          line-height: 1;
          transition: color 0.15s;
        }
        .srd-close:hover { color: var(--text-primary, #c9d1d9); }

        .srd-body {
          padding: 16px 20px;
          overflow-y: auto;
          flex: 1;
          display: flex;
          flex-direction: column;
          gap: 14px;
        }

        .srd-error {
          padding: 8px 12px;
          border-radius: 6px;
          background: rgba(248, 81, 73, 0.1);
          border: 1px solid rgba(248, 81, 73, 0.35);
          color: #f85149;
          font-size: 13px;
          margin: 0;
        }

        .srd-create-row {
          display: flex;
        }

        .srd-create-btn {
          padding: 7px 16px;
          border-radius: 6px;
          font-size: 13px;
          font-weight: 500;
          background: var(--accent, #238636);
          color: #fff;
          border: none;
          cursor: pointer;
          transition: opacity 0.15s;
        }
        .srd-create-btn:hover:not(:disabled) { opacity: 0.85; }
        .srd-create-btn:disabled { opacity: 0.5; cursor: default; }

        .srd-token-box {
          background: var(--bg-primary, #0d1117);
          border: 1px solid var(--border, #30363d);
          border-radius: 6px;
          padding: 12px;
          display: flex;
          flex-direction: column;
          gap: 8px;
        }

        .srd-token-hint {
          font-size: 12px;
          color: var(--text-secondary, #8b949e);
          margin: 0;
        }

        .srd-token-row {
          display: flex;
          gap: 8px;
        }

        .srd-token-input {
          flex: 1;
          font-family: var(--font-mono, monospace);
          font-size: 12px;
          padding: 6px 10px;
          border-radius: 5px;
          border: 1px solid var(--border, #30363d);
          background: var(--bg-secondary, #161b22);
          color: var(--text-primary, #c9d1d9);
          min-width: 0;
        }

        .srd-copy-btn {
          padding: 6px 14px;
          border-radius: 5px;
          font-size: 12px;
          font-weight: 500;
          border: 1px solid var(--border, #30363d);
          background: transparent;
          color: var(--text-primary, #c9d1d9);
          cursor: pointer;
          flex-shrink: 0;
          transition: background 0.15s;
        }
        .srd-copy-btn:hover { background: rgba(255,255,255,0.06); }

        .srd-shares-section {
          display: flex;
          flex-direction: column;
          gap: 8px;
        }

        .srd-section-title {
          font-size: 12px;
          font-weight: 600;
          text-transform: uppercase;
          letter-spacing: 0.06em;
          color: var(--text-secondary, #8b949e);
          margin: 0;
        }

        .srd-muted {
          color: var(--text-secondary, #8b949e);
          font-size: 13px;
          margin: 0;
          font-style: italic;
        }

        .srd-share-list {
          list-style: none;
          margin: 0;
          padding: 0;
          display: flex;
          flex-direction: column;
          gap: 6px;
        }

        .srd-share-item {
          display: flex;
          align-items: center;
          justify-content: space-between;
          gap: 12px;
          padding: 8px 10px;
          border-radius: 6px;
          border: 1px solid var(--border, #30363d);
          background: var(--bg-primary, #0d1117);
        }

        .srd-share-info {
          display: flex;
          flex-wrap: wrap;
          align-items: center;
          gap: 8px;
          min-width: 0;
        }

        .srd-share-prefix {
          font-size: 12px;
          color: var(--text-secondary, #8b949e);
        }

        .srd-share-meta {
          font-size: 12px;
          color: var(--text-primary, #c9d1d9);
        }

        .srd-share-expiry {
          font-size: 11px;
          color: var(--warning, #d29922);
        }

        .srd-share-created {
          font-size: 11px;
          color: var(--text-secondary, #8b949e);
        }

        .mono { font-family: var(--font-mono, monospace); }

        .srd-revoke-btn {
          padding: 4px 12px;
          border-radius: 5px;
          font-size: 12px;
          border: 1px solid rgba(248, 81, 73, 0.4);
          background: transparent;
          color: #f85149;
          cursor: pointer;
          flex-shrink: 0;
          transition: background 0.15s, color 0.15s;
          white-space: nowrap;
        }
        .srd-revoke-btn:hover:not(:disabled) {
          background: #f85149;
          color: #fff;
        }
        .srd-revoke-btn:disabled { opacity: 0.5; cursor: default; }
      `}</style>
    </div>
  );
}
