// web/src/pages/Recordings.tsx
import { createSignal, onMount, Show, For, createMemo } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { api, errorMessage } from '../lib/api';
import { formatBytes, formatDate } from '../lib/format';
import type { Recording } from '../lib/protocol';
import { auth } from '../stores/auth';
import { toast } from '../stores/toast';
import Banner from '../components/Banner';
import LocaleSwitcher from '../components/LocaleSwitcher';

type StatusFilter = 'all' | 'completed' | 'recording' | 'failed';

const STATUS_FILTERS: StatusFilter[] = ['all', 'completed', 'recording', 'failed'];

function formatDurationMs(ms: number | null): string {
  if (ms == null || ms <= 0) return '--';
  const sec = Math.floor(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ${sec % 60}s`;
  const hr = Math.floor(min / 60);
  return `${hr}h ${min % 60}m`;
}

export default function Recordings() {
  const navigate = useNavigate();

  const [recordings, setRecordings] = createSignal<Recording[]>([]);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal('');
  const [statusFilter, setStatusFilter] = createSignal<StatusFilter>('all');
  const [busyId, setBusyId] = createSignal<string | null>(null);

  onMount(async () => {
    await loadRecordings();
  });

  async function loadRecordings() {
    setLoading(true);
    setError('');
    try {
      const data = await api.listRecordings();
      setRecordings(data);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  const filtered = createMemo(() => {
    const filter = statusFilter();
    const all = recordings();
    if (filter === 'all') return all;
    return all.filter((r) => r.status === filter);
  });

  async function handleDelete(r: Recording) {
    if (busyId()) return;
    if (!confirm(`Delete recording ${r.id}? This cannot be undone.`)) return;
    setBusyId(r.id);
    try {
      await api.deleteRecording(r.id);
      setRecordings((prev) => prev.filter((x) => x.id !== r.id));
      toast.success('Recording deleted');
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  async function handleKeep(r: Recording) {
    if (busyId()) return;
    setBusyId(r.id);
    try {
      const updated = await api.keepRecording(r.id);
      setRecordings((prev) => prev.map((x) => (x.id === updated.id ? updated : x)));
      toast.success('Recording kept (no expiry)');
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  async function handleExpire(r: Recording) {
    if (busyId()) return;
    setBusyId(r.id);
    try {
      const updated = await api.expireRecording(r.id);
      setRecordings((prev) => prev.map((x) => (x.id === updated.id ? updated : x)));
      toast.success('Recording scheduled for expiry');
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  function statusBadgeData(status: Recording['status']): { label: string; cls: string } {
    switch (status) {
      case 'completed':
        return { label: 'Completed', cls: 'badge-completed' };
      case 'recording':
        return { label: 'Recording', cls: 'badge-recording' };
      case 'failed':
        return { label: 'Failed', cls: 'badge-failed' };
    }
  }

  return (
    <div class="recordings-page">
      <header class="topbar">
        <div class="topbar-left">
          <a class="back-link" href="/">&#8592; Dashboard</a>
          <h1>Recordings</h1>
        </div>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
          <button
            class="refresh-btn"
            onClick={loadRecordings}
            disabled={loading()}
            title="Refresh"
          >
            {loading() ? 'Refreshing…' : 'Refresh'}
          </button>
          <button onClick={() => auth.logout()}>Logout</button>
        </div>
      </header>

      <Show when={error()}>
        <Banner variant="error" onDismiss={() => setError('')}>
          {error()}
        </Banner>
      </Show>

      <main class="content">
        {/* Status filter tabs */}
        <div class="filter-tabs" role="tablist" aria-label="Status filter">
          <For each={STATUS_FILTERS}>
            {(f) => (
              <button
                type="button"
                class="filter-tab"
                role="tab"
                aria-selected={statusFilter() === f}
                onClick={() => setStatusFilter(f)}
              >
                {f === 'all' ? 'All' : f.charAt(0).toUpperCase() + f.slice(1)}
              </button>
            )}
          </For>
        </div>

        <Show
          when={!loading()}
          fallback={
            <div class="loading-state">
              <p class="muted">Loading recordings…</p>
            </div>
          }
        >
          <Show
            when={filtered().length > 0}
            fallback={
              <div class="empty-state">
                <p class="empty-title">No recordings found</p>
                <p class="empty-body">
                  {statusFilter() === 'all'
                    ? 'Start a session and click "Start Recording" to create one.'
                    : `No recordings with status "${statusFilter()}".`}
                </p>
              </div>
            }
          >
            <div class="recordings-list">
              <For each={filtered()}>
                {(rec) => {
                  const badge = () => statusBadgeData(rec.status);
                  const busy = () => busyId() === rec.id;
                  return (
                    <div class="recording-card" data-status={rec.status}>
                      <div class="rec-header">
                        <span class={`rec-badge ${badge().cls}`}>{badge().label}</span>
                        <span class="rec-id">{rec.id}</span>
                        <Show when={rec.expires_at}>
                          <span class="rec-expires" title={`Expires: ${rec.expires_at}`}>
                            Expires {formatDate(rec.expires_at!)}
                          </span>
                        </Show>
                      </div>

                      <div class="rec-body">
                        <div class="rec-meta-grid">
                          <span class="meta-label">Session</span>
                          <span class="meta-value mono">{rec.session_id}</span>

                          <span class="meta-label">Duration</span>
                          <span class="meta-value">{formatDurationMs(rec.duration_ms)}</span>

                          <span class="meta-label">Size</span>
                          <span class="meta-value">{formatBytes(rec.file_size)}</span>

                          <span class="meta-label">Dimensions</span>
                          <span class="meta-value">{rec.width}×{rec.height}</span>

                          <span class="meta-label">Events</span>
                          <span class="meta-value">{rec.event_count.toLocaleString()}</span>

                          <span class="meta-label">Started</span>
                          <span class="meta-value">{formatDate(rec.started_at)}</span>

                          <Show when={rec.completed_at}>
                            <span class="meta-label">Completed</span>
                            <span class="meta-value">{formatDate(rec.completed_at!)}</span>
                          </Show>
                        </div>
                      </div>

                      <div class="rec-actions">
                        <Show when={rec.status === 'completed'}>
                          <button
                            type="button"
                            class="action-btn action-play"
                            onClick={() => navigate(`/recordings/${rec.id}`)}
                            disabled={busy()}
                          >
                            ▶ Play
                          </button>
                        </Show>

                        <Show when={rec.expires_at}>
                          <button
                            type="button"
                            class="action-btn action-keep"
                            onClick={() => handleKeep(rec)}
                            disabled={busy()}
                            title="Remove expiry — keep this recording forever"
                          >
                            Keep
                          </button>
                        </Show>

                        <Show when={!rec.expires_at && rec.status === 'completed'}>
                          <button
                            type="button"
                            class="action-btn action-expire"
                            onClick={() => handleExpire(rec)}
                            disabled={busy()}
                            title="Schedule for expiry"
                          >
                            Set TTL
                          </button>
                        </Show>

                        <button
                          type="button"
                          class="action-btn action-delete"
                          onClick={() => handleDelete(rec)}
                          disabled={busy()}
                        >
                          Delete
                        </button>
                      </div>
                    </div>
                  );
                }}
              </For>
            </div>
          </Show>
        </Show>
      </main>

      <style>{`
        .recordings-page { min-height: 100vh; }

        .topbar {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 12px 24px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
        }
        .topbar-left {
          display: flex;
          align-items: center;
          gap: 16px;
        }
        .topbar h1 { font-size: 18px; font-weight: 700; }
        .topbar-actions { display: flex; gap: 8px; align-items: center; }
        .back-link {
          font-size: 13px;
          color: var(--text-secondary);
          text-decoration: none;
          transition: color 0.15s;
        }
        .back-link:hover { color: var(--text-primary); }
        .refresh-btn:disabled { opacity: 0.6; cursor: default; }

        .content { padding: 24px; max-width: 960px; margin: 0 auto; }

        .filter-tabs {
          display: flex;
          gap: 4px;
          margin-bottom: 16px;
        }
        .filter-tab {
          background: transparent;
          border: 1px solid var(--border);
          color: var(--text-secondary);
          padding: 6px 14px;
          border-radius: 999px;
          font: inherit;
          font-size: 13px;
          cursor: pointer;
          transition: all 0.15s;
        }
        .filter-tab:hover { border-color: var(--accent); color: var(--text-primary); }
        .filter-tab[aria-selected='true'] {
          background: var(--accent);
          border-color: var(--accent);
          color: var(--bg-primary);
        }

        .loading-state, .empty-state {
          border: 1px dashed var(--border);
          border-radius: 8px;
          padding: 24px;
          background: var(--bg-secondary);
          text-align: center;
        }
        .empty-title { font-weight: 600; margin-bottom: 6px; color: var(--text-primary); }
        .empty-body { color: var(--text-secondary); font-size: 13px; }
        .muted { color: var(--text-secondary); font-size: 14px; }

        .recordings-list { display: flex; flex-direction: column; gap: 10px; }

        .recording-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 8px;
          padding: 14px 16px;
          transition: border-color 0.15s;
        }
        .recording-card[data-status='failed'] { opacity: 0.7; }

        .rec-header {
          display: flex;
          align-items: center;
          gap: 10px;
          margin-bottom: 10px;
        }
        .rec-id {
          font-family: var(--font-mono);
          font-size: 12px;
          color: var(--accent);
          flex: 1;
        }
        .rec-expires {
          font-size: 11px;
          color: var(--warning, #d29922);
          border: 1px solid var(--warning, #d29922);
          padding: 2px 8px;
          border-radius: 10px;
        }

        .rec-badge {
          display: inline-block;
          font-size: 11px;
          font-weight: 600;
          padding: 2px 8px;
          border-radius: 10px;
          border: 1px solid;
          text-transform: uppercase;
          letter-spacing: 0.04em;
        }
        .badge-completed { color: var(--success, #3fb950); border-color: var(--success, #3fb950); }
        .badge-recording { color: #f88; border-color: #f88; }
        .badge-failed    { color: var(--error, #f85149); border-color: var(--error, #f85149); }

        .rec-body { margin-bottom: 12px; }
        .rec-meta-grid {
          display: grid;
          grid-template-columns: 90px 1fr;
          gap: 4px 12px;
          font-size: 13px;
        }
        .meta-label { color: var(--text-secondary); }
        .meta-value { color: var(--text-primary); }
        .mono { font-family: var(--font-mono); font-size: 12px; }

        .rec-actions {
          display: flex;
          gap: 8px;
          flex-wrap: wrap;
        }
        .action-btn {
          font: inherit;
          font-size: 12px;
          padding: 5px 12px;
          border-radius: 6px;
          cursor: pointer;
          border: 1px solid var(--border);
          background: transparent;
          color: var(--text-secondary);
          transition: all 0.15s;
        }
        .action-btn:hover:not(:disabled) { border-color: var(--accent); color: var(--text-primary); }
        .action-btn:disabled { opacity: 0.5; cursor: default; }
        .action-play {
          border-color: var(--accent);
          color: var(--accent);
          font-weight: 600;
        }
        .action-play:hover:not(:disabled) { background: var(--accent); color: var(--bg-primary); }
        .action-delete { color: var(--error, #f85149); border-color: var(--error, #f85149); }
        .action-delete:hover:not(:disabled) { background: var(--error, #f85149); color: var(--bg-primary); }
      `}</style>
    </div>
  );
}
