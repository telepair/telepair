// web/src/pages/Dashboard.tsx
import { createSignal, onMount, Show, For } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { sessionStore } from '../stores/session';
import type { InputMode, TargetInfo } from '../lib/protocol';
import Banner from '../components/Banner';
import { TargetCardSkeleton } from '../components/Skeleton';
import CreateSessionDialog from '../components/CreateSessionDialog';

export default function Dashboard() {
  const navigate = useNavigate();
  const [launchError, setLaunchError] = createSignal('');
  // Remember the last mode the user picked so the next dialog opens on
  // their preferred default instead of forcing them to re-toggle every
  // time. Persisting across reloads would be nice but isn't worth a
  // dedicated storage key — an in-memory signal is good enough.
  const [lastMode, setLastMode] = createSignal<InputMode>('multiplexed');
  const [pendingTarget, setPendingTarget] = createSignal<TargetInfo | null>(null);
  const [launching, setLaunching] = createSignal(false);

  onMount(() => {
    sessionStore.refresh();
  });

  const handleCardClick = (target: TargetInfo) => {
    setLaunchError('');
    setPendingTarget(target);
  };

  const handleCancelLaunch = () => {
    if (launching()) return;
    setPendingTarget(null);
  };

  const handleConfirmLaunch = async (mode: InputMode) => {
    const target = pendingTarget();
    if (!target || launching()) return;
    setLaunching(true);
    setLastMode(mode);
    try {
      const session = await sessionStore.createSession(target.name, mode);
      setPendingTarget(null);
      navigate(`/session/${session.id}`);
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to create session';
      setLaunchError(msg);
      setPendingTarget(null);
    } finally {
      setLaunching(false);
    }
  };

  const handleRefresh = () => {
    setLaunchError('');
    sessionStore.refresh();
  };

  return (
    <div class="dashboard">
      <header class="topbar">
        <h1>telepair</h1>
        <div class="topbar-actions">
          <button
            class="refresh-btn"
            onClick={handleRefresh}
            disabled={sessionStore.loading()}
            aria-label="Refresh targets and sessions"
            title="Refresh"
          >
            {sessionStore.loading() ? 'Refreshing…' : 'Refresh'}
          </button>
          <button onClick={() => auth.logout()}>Logout</button>
        </div>
      </header>

      <Show when={launchError()}>
        <Banner variant="error" onDismiss={() => setLaunchError('')}>
          {launchError()}
        </Banner>
      </Show>

      <main class="content">
        <section>
          <div class="section-header">
            <h2>Targets</h2>
            <p class="section-hint">Click a target to configure mode and launch.</p>
          </div>
          <Show
            when={!sessionStore.loading()}
            fallback={
              <div class="target-grid">
                <For each={Array.from({ length: 6 })}>
                  {() => <TargetCardSkeleton />}
                </For>
              </div>
            }
          >
            <Show
              when={sessionStore.targets().length > 0}
              fallback={
                <div class="empty-state">
                  <p class="empty-title">No targets available</p>
                  <p class="empty-body">
                    No targets are configured for this account. If you expected
                    to see one here, contact your administrator or check the
                    server logs.
                  </p>
                </div>
              }
            >
              <div class="target-grid">
                <For each={sessionStore.targets()}>
                  {(target) => (
                    <button
                      type="button"
                      class="target-card"
                      onClick={() => handleCardClick(target)}
                    >
                      <div class="target-name">{target.display}</div>
                      <div class="target-id">{target.name}</div>
                      <Show when={target.tags.length > 0}>
                        <div class="tags">
                          <For each={target.tags}>
                            {(tag) => <span class="tag">{tag}</span>}
                          </For>
                        </div>
                      </Show>
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </Show>
        </section>

        <section>
          <h2>Active Sessions</h2>
          <Show when={sessionStore.sessions().length > 0} fallback={<p class="muted">No active sessions</p>}>
            <div class="session-list">
              <For each={sessionStore.sessions()}>
                {(session) => (
                  <div class="session-row" onClick={() => navigate(`/session/${session.id}`)}>
                    <span class="session-id">{session.id}</span>
                    <span class="session-target">{session.target_name}</span>
                    <span class="session-mode">{session.input_mode}</span>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </section>
      </main>

      <CreateSessionDialog
        target={pendingTarget()}
        defaultMode={lastMode()}
        busy={launching()}
        onCancel={handleCancelLaunch}
        onLaunch={handleConfirmLaunch}
      />

      <style>{`
        .dashboard { min-height: 100vh; }
        .topbar {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 12px 24px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
        }
        .topbar h1 { font-size: 18px; font-weight: 700; }
        .topbar-actions { display: flex; gap: 8px; align-items: center; }
        .refresh-btn:disabled {
          opacity: 0.6;
          cursor: default;
        }
        .content { padding: 24px; max-width: 960px; margin: 0 auto; }
        .content h2 { font-size: 16px; font-weight: 600; color: var(--text-secondary); }
        .content section { margin-bottom: 32px; }
        .section-header {
          display: flex;
          align-items: baseline;
          justify-content: space-between;
          margin-bottom: 12px;
          gap: 16px;
        }
        .section-hint {
          font-size: 12px;
          color: var(--text-secondary);
        }
        .muted { color: var(--text-secondary); font-size: 14px; }
        .empty-state {
          border: 1px dashed var(--border);
          border-radius: 8px;
          padding: 20px 24px;
          background: var(--bg-secondary);
        }
        .empty-title {
          font-weight: 600;
          margin-bottom: 6px;
          color: var(--text-primary);
        }
        .empty-body {
          color: var(--text-secondary);
          font-size: 13px;
          line-height: 1.6;
        }
        .empty-body code {
          font-family: var(--font-mono);
          font-size: 12px;
          padding: 1px 6px;
          background: var(--bg-tertiary);
          border-radius: 4px;
        }

        .target-grid {
          display: grid;
          grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
          gap: 12px;
        }
        .target-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 8px;
          padding: 16px;
          cursor: pointer;
          transition: border-color 0.15s;
          text-align: left;
          font: inherit;
          color: inherit;
        }
        .target-card:hover { border-color: var(--accent); }
        .target-card:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
        .target-name { font-weight: 600; margin-bottom: 4px; }
        .target-id { font-family: var(--font-mono); font-size: 12px; color: var(--text-secondary); }
        .tags { margin-top: 8px; display: flex; gap: 4px; flex-wrap: wrap; }
        .tag {
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 12px;
          background: var(--bg-tertiary);
          color: var(--text-secondary);
        }

        .session-list { display: flex; flex-direction: column; gap: 4px; }
        .session-row {
          display: flex;
          gap: 16px;
          padding: 10px 14px;
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 6px;
          cursor: pointer;
          font-size: 14px;
          transition: border-color 0.15s;
        }
        .session-row:hover { border-color: var(--accent); }
        .session-id { font-family: var(--font-mono); color: var(--accent); min-width: 100px; }
        .session-target { color: var(--text-primary); }
        .session-mode { color: var(--text-secondary); margin-left: auto; font-size: 12px; }
      `}</style>
    </div>
  );
}
