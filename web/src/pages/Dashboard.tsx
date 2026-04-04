// web/src/pages/Dashboard.tsx
import { onMount, Show, For } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { sessionStore } from '../stores/session';

export default function Dashboard() {
  const navigate = useNavigate();

  onMount(() => {
    sessionStore.refresh();
  });

  const handleLaunch = async (targetName: string) => {
    try {
      const session = await sessionStore.createSession(targetName);
      navigate(`/session/${session.id}`);
    } catch (e) {
      console.error('Failed to create session:', e);
    }
  };

  return (
    <div class="dashboard">
      <header class="topbar">
        <h1>telepair</h1>
        <button onClick={() => auth.logout()}>Logout</button>
      </header>

      <main class="content">
        <section>
          <h2>Targets</h2>
          <Show when={!sessionStore.loading()} fallback={<p class="muted">Loading...</p>}>
            <div class="target-grid">
              <For each={sessionStore.targets()} fallback={<p class="muted">No targets configured</p>}>
                {(target) => (
                  <div class="target-card" onClick={() => handleLaunch(target.name)}>
                    <div class="target-name">{target.display}</div>
                    <div class="target-id">{target.name}</div>
                    <Show when={target.tags.length > 0}>
                      <div class="tags">
                        <For each={target.tags}>
                          {(tag) => <span class="tag">{tag}</span>}
                        </For>
                      </div>
                    </Show>
                  </div>
                )}
              </For>
            </div>
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
        .content { padding: 24px; max-width: 960px; margin: 0 auto; }
        .content h2 { font-size: 16px; font-weight: 600; margin-bottom: 12px; color: var(--text-secondary); }
        .content section { margin-bottom: 32px; }
        .muted { color: var(--text-secondary); font-size: 14px; }

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
        }
        .target-card:hover { border-color: var(--accent); }
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
