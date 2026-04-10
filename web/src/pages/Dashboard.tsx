// web/src/pages/Dashboard.tsx
import { createSignal, onMount, Show, For, createMemo, createEffect } from 'solid-js';
import { useNavigate, useSearchParams } from '@solidjs/router';
import { auth } from '../stores/auth';
import { sessionStore, type SessionsFilter } from '../stores/session';
import type { CloseReason, InputMode, Session, TargetInfo } from '../lib/protocol';
import Banner from '../components/Banner';
import { TargetCardSkeleton } from '../components/Skeleton';
import CreateSessionDialog from '../components/CreateSessionDialog';
import SessionDetailDialog from '../components/SessionDetailDialog';
import LocaleSwitcher from '../components/LocaleSwitcher';
import { inputModeLabel, useI18n, type Translator } from '../i18n';

/** Narrow the free-form `status` query param to a valid tab value.
 *  Anything else (or absent) falls back to the legacy default so a
 *  stray URL typo doesn't trap the user on a blank tab. */
function coerceFilter(raw: string | undefined): SessionsFilter {
  if (raw === 'active' || raw === 'closed' || raw === 'all') return raw;
  return 'active';
}

// Ordered list of filter tabs. Kept at module level so the <For> that
// renders the chip row gets a stable reference — a fresh array on every
// render would force Solid to re-create the DOM nodes each time.
const SESSION_TABS: SessionsFilter[] = ['active', 'closed', 'all'];

export default function Dashboard() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams<{
    target?: string;
    status?: string;
  }>();
  const [launchError, setLaunchError] = createSignal('');
  // Remember the last mode the user picked so the next dialog opens on
  // their preferred default instead of forcing them to re-toggle every
  // time. Persisting across reloads would be nice but isn't worth a
  // dedicated storage key — an in-memory signal is good enough.
  const [lastMode, setLastMode] = createSignal<InputMode>('multiplexed');
  const [pendingTarget, setPendingTarget] = createSignal<TargetInfo | null>(null);
  const [launching, setLaunching] = createSignal(false);
  // Selected session for the audit-timeline dialog. `null` keeps the
  // dialog closed; setting a closed-session row opens it. Active rows
  // still navigate to the live session page below — the dialog is the
  // *history* surface, not a generic detail view.
  const [detailSession, setDetailSession] = createSignal<Session | null>(null);

  onMount(() => {
    // Identity load is fire-and-forget: the dashboard's owner-gate
    // (in `handleSessionClick`) reads `auth.currentUserId()` lazily,
    // so a slow whoami won't block the first paint. The signal lands
    // before any user click in practice, and worst case the first
    // click on a freshly-loaded page no-ops gracefully (then becomes
    // clickable on the next render once the signal flips). Doing
    // this from `validateToken` covers the login flow already; this
    // mount-time call covers the "open the dashboard with a token
    // already in storage" reload path.
    auth.loadIdentity();
    // First fetch honours the URL query params so a deep link from
    // the admin targets page (`/?target=alpha&status=active`) lands
    // on the right tab with the right filter. Subsequent changes to
    // the query string are handled by the createEffect below.
    sessionStore.fetchTargets();
    sessionStore.fetchSessions(
      coerceFilter(searchParams.status),
      searchParams.target ?? '',
    );
  });

  // Reactive URL → store sync. Fires whenever the user navigates
  // (back/forward, clicking a new deep link, clearing filters via
  // `setSearchParams`) so the list and the URL stay in lockstep.
  // Guarded against no-op updates because the store's setters are
  // themselves signals — writing the same value would still notify
  // subscribers but would also trigger an unnecessary API call.
  createEffect(() => {
    const nextStatus = coerceFilter(searchParams.status);
    const nextTarget = searchParams.target ?? '';
    if (
      nextStatus === sessionStore.currentFilter() &&
      nextTarget === sessionStore.currentTargetFilter()
    ) {
      return;
    }
    sessionStore.fetchSessions(nextStatus, nextTarget);
  });

  // Whether the current user owns `session`. The dashboard's session
  // list mixes "owned" and "merely joined" rows (the SQL is
  // `WHERE owner_id = ? OR p.user_id IS NOT NULL`), so we can't
  // assume every row is ours. Compared as strings — both sides come
  // from the same Uuid serialization, so a parse round-trip is
  // unnecessary. Returns false until `loadIdentity()` lands so the
  // gate is fail-closed during the first paint window.
  const isOwner = (session: Session): boolean => {
    const me = auth.currentUserId();
    return me.length > 0 && session.owner_id === me;
  };

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
      // Server errors come back in English from the API; only the
      // local fallback (non-Error throws) needs translating.
      const msg = e instanceof Error ? e.message : t('create_session.error_failed');
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

  const handleTabClick = (tab: SessionsFilter) => {
    if (sessionStore.currentFilter() === tab) return;
    // Drive the URL — the createEffect above reads the new params
    // back and issues the fetch. Writing the URL first keeps the
    // browser history honest (back button undoes a tab switch) and
    // is the single place that owns the "filter → URL" projection.
    setSearchParams(
      { status: tab, target: sessionStore.currentTargetFilter() || undefined },
      { replace: false },
    );
  };

  const handleClearTargetFilter = () => {
    setSearchParams(
      { status: sessionStore.currentFilter(), target: undefined },
      { replace: false },
    );
  };

  const handleSessionClick = (session: Session) => {
    // Active rows jump straight back into the live session page —
    // every visible active row is one the user is a participant of,
    // so the navigation always succeeds (the live page handles the
    // role check itself).
    //
    // Closed rows open the audit-timeline detail dialog *only* if
    // the current user owns the row. The dialog's audit fetch hits
    // the owner-only `/api/sessions/:id/audit` endpoint, and the
    // dashboard list deliberately surfaces sessions the user merely
    // joined alongside ones they own — without this gate, clicking a
    // joined-but-not-owned closed row would deterministically 403
    // and surface as an in-dialog error banner. Better UX is to
    // leave those rows inert (matches their pre-Stage-5d behavior).
    if (session.status === 'closed') {
      if (!isOwner(session)) return;
      setDetailSession(session);
      return;
    }
    navigate(`/session/${session.id}`);
  };

  const tabLabel = (tab: SessionsFilter): string => {
    switch (tab) {
      case 'active':
        return t('dashboard.sessions_tab_active');
      case 'closed':
        return t('dashboard.sessions_tab_closed');
      case 'all':
        return t('dashboard.sessions_tab_all');
    }
  };

  const emptyLabel = (): string => {
    switch (sessionStore.currentFilter()) {
      case 'active':
        return t('dashboard.sessions_empty_active');
      case 'closed':
        return t('dashboard.sessions_empty_closed');
      case 'all':
        return t('dashboard.sessions_empty_all');
    }
  };

  // Memoised so the underlying array reference is stable when the
  // filter/sessions signals haven't changed — otherwise the For tag
  // below would treat every render as "new rows" and thrash.
  const orderedSessions = createMemo(() => {
    // Newest first: closed rows by closed_at, active rows by
    // created_at. We can't just sort by created_at because a session
    // that was live for an hour should sit above one that started
    // yesterday and closed immediately.
    return [...sessionStore.sessions()].sort((a, b) => {
      const ta = Date.parse(a.closed_at ?? a.created_at);
      const tb = Date.parse(b.closed_at ?? b.created_at);
      return tb - ta;
    });
  });

  return (
    <div class="dashboard">
      <header class="topbar">
        <h1>telepair</h1>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
          {/*
            Admin-only entry point into the target management page.
            The guard uses the three-state `currentUserIsAdmin()` —
            the icon only renders when whoami has confirmed admin,
            never on the `null` (loading) or `false` states. That
            closes the "flash of admin UI" window on first paint
            for non-admin users.
          */}
          <Show when={auth.currentUserIsAdmin() === true}>
            <a
              class="admin-link"
              href="/admin/targets"
              aria-label={t('dashboard.admin_targets_link_aria')}
              data-testid="admin-targets-link"
            >
              {t('dashboard.admin_targets_link')}
            </a>
          </Show>
          <button
            class="refresh-btn"
            onClick={handleRefresh}
            disabled={sessionStore.loading()}
            aria-label={t('dashboard.refresh_aria')}
            title={t('common.refresh')}
          >
            {sessionStore.loading() ? t('common.refreshing') : t('common.refresh')}
          </button>
          <button onClick={() => auth.logout()}>{t('common.logout')}</button>
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
            <h2>{t('dashboard.targets_heading')}</h2>
            <p class="section-hint">{t('dashboard.targets_hint')}</p>
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
                  <p class="empty-title">{t('dashboard.targets_empty_title')}</p>
                  <p class="empty-body">{t('dashboard.targets_empty_body')}</p>
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
          <div class="section-header">
            <h2>{t('dashboard.sessions_heading')}</h2>
            <p class="section-hint">{t('dashboard.sessions_hint')}</p>
          </div>
          <div class="session-tabs" role="tablist" aria-label={t('dashboard.sessions_heading')}>
            <For each={SESSION_TABS}>
              {(tab) => (
                <button
                  type="button"
                  class="session-tab"
                  role="tab"
                  aria-selected={sessionStore.currentFilter() === tab}
                  data-tab={tab}
                  onClick={() => handleTabClick(tab)}
                >
                  {tabLabel(tab)}
                </button>
              )}
            </For>
            <Show when={sessionStore.currentTargetFilter()}>
              {/*
                Target filter chip surfaces the `?target=` URL param
                so the user can see at a glance why they only have a
                subset of rows. Clicking the chip clears the filter
                (and the URL param) — the chip is the *only* UI to
                remove the filter, so it needs to stay visible the
                whole time the filter is active.
              */}
              <button
                type="button"
                class="session-tab target-chip"
                data-testid="session-target-filter-chip"
                onClick={handleClearTargetFilter}
                title={t('dashboard.sessions_filter_clear')}
              >
                {t('dashboard.sessions_filter_target', {
                  name: sessionStore.currentTargetFilter(),
                })}
                <span class="target-chip-close" aria-hidden="true">×</span>
              </button>
            </Show>
          </div>
          <Show
            when={orderedSessions().length > 0}
            fallback={<p class="muted">{emptyLabel()}</p>}
          >
            <div class="session-list">
              <For each={orderedSessions()}>
                {(session) => (
                  <SessionRow
                    session={session}
                    // Active rows are always clickable (the live page
                    // handles the auth check). Closed rows are
                    // clickable only when the caller owns them — see
                    // `handleSessionClick` for the gate rationale.
                    // The clickability flips reactively once
                    // `auth.currentUserId()` lands, so a row that
                    // mounted in fail-closed mode upgrades to
                    // clickable on the next paint without a refresh.
                    clickable={session.status === 'active' || isOwner(session)}
                    onClick={() => handleSessionClick(session)}
                  />
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

      <SessionDetailDialog
        session={detailSession()}
        onClose={() => setDetailSession(null)}
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
        .admin-link {
          display: inline-flex;
          align-items: center;
          padding: 6px 12px;
          border: 1px solid var(--border);
          border-radius: 999px;
          font-size: 13px;
          color: var(--text-secondary);
          text-decoration: none;
          transition: all 0.15s;
        }
        .admin-link:hover {
          border-color: var(--accent);
          color: var(--text-primary);
        }
        .admin-link:focus-visible {
          outline: 2px solid var(--accent);
          outline-offset: 2px;
        }
        .target-chip {
          display: inline-flex;
          align-items: center;
          gap: 6px;
          border-color: var(--accent);
          color: var(--text-primary);
        }
        .target-chip-close {
          font-size: 14px;
          line-height: 1;
          color: var(--text-secondary);
        }
        .target-chip:hover .target-chip-close {
          color: var(--text-primary);
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

        .session-tabs {
          display: flex;
          gap: 4px;
          margin-bottom: 12px;
        }
        .session-tab {
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
        .session-tab:hover { border-color: var(--accent); color: var(--text-primary); }
        .session-tab[aria-selected='true'] {
          background: var(--accent);
          border-color: var(--accent);
          color: var(--bg-primary);
        }

        .session-list { display: flex; flex-direction: column; gap: 4px; }
        .session-row {
          display: flex;
          gap: 16px;
          padding: 10px 14px;
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 6px;
          font-size: 14px;
          transition: border-color 0.15s;
          align-items: center;
        }
        .session-row[data-clickable='true'] { cursor: pointer; }
        .session-row[data-clickable='true']:hover { border-color: var(--accent); }
        .session-row[data-status='closed'] { opacity: 0.82; }
        .session-id { font-family: var(--font-mono); color: var(--accent); min-width: 100px; }
        .session-target { color: var(--text-primary); }
        .session-mode { color: var(--text-secondary); font-size: 12px; }
        .session-meta {
          margin-left: auto;
          display: flex;
          gap: 8px;
          align-items: center;
          font-size: 12px;
          color: var(--text-secondary);
        }
        .session-duration { font-variant-numeric: tabular-nums; }
        .session-reason {
          padding: 2px 8px;
          border-radius: 10px;
          font-size: 11px;
          border: 1px solid var(--border);
        }
        .session-reason[data-reason='owner']   { color: var(--text-primary); }
        .session-reason[data-reason='reaper']  { color: #c88; border-color: #c88; }
        .session-reason[data-reason='startup'] { color: #cc8; border-color: #cc8; }
        .session-reason[data-reason='error']   { color: #f88; border-color: #f88; }
        .session-reason[data-reason='active']  { color: #8c8; border-color: #8c8; }
      `}</style>
    </div>
  );
}

// --- row component ------------------------------------------------------

interface SessionRowProps {
  session: Session;
  /** When false the row gets no hover style and ignores clicks — used
   *  for closed-but-not-owned rows where the audit dialog would 403. */
  clickable: boolean;
  onClick: () => void;
}

/** Pretty-print the gap between `start` and `end` as a short,
 *  locale-aware duration chip. Degrades to empty string if either
 *  timestamp is missing or unparseable — the column is purely
 *  informational, so we'd rather show nothing than a crash. */
function formatDuration(start: string, end: string | null, t: Translator): string {
  if (!end) return '';
  const delta = Date.parse(end) - Date.parse(start);
  if (!Number.isFinite(delta) || delta < 0) return '';
  const seconds = Math.floor(delta / 1000);
  if (seconds < 60) return t('dashboard.session_duration_sec', { n: String(seconds) });
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return t('dashboard.session_duration_min', { n: String(minutes) });
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return t('dashboard.session_duration_hr', {
    h: String(hours),
    m: String(remainder),
  });
}

function reasonLabel(
  session: Session,
  t: Translator,
): { label: string; data: string } {
  if (session.status === 'active') {
    return { label: t('dashboard.sessions_status_active'), data: 'active' };
  }
  const reason: CloseReason | null | undefined = session.closed_reason;
  switch (reason) {
    case 'owner':
      return { label: t('dashboard.sessions_closed_by_owner'), data: 'owner' };
    case 'reaper':
      return { label: t('dashboard.sessions_closed_by_reaper'), data: 'reaper' };
    case 'startup':
      return { label: t('dashboard.sessions_closed_by_startup'), data: 'startup' };
    case 'error':
      return { label: t('dashboard.sessions_closed_by_error'), data: 'error' };
    default:
      return { label: t('dashboard.sessions_closed_unknown'), data: 'closed' };
  }
}

function SessionRow(props: SessionRowProps) {
  const { t } = useI18n();
  const isClosed = () => props.session.status === 'closed';
  const reason = () => reasonLabel(props.session, t);
  const duration = () => formatDuration(props.session.created_at, props.session.closed_at, t);

  return (
    <div
      class="session-row"
      data-status={props.session.status}
      // Active rows jump back into the live session; owned closed
      // rows open the audit-timeline dialog. Non-owned closed rows
      // are inert — the `data-clickable='true'` attribute is the only
      // selector for the hover/cursor CSS rules below, so a `false`
      // row gets neither the pointer cursor nor the accent border.
      data-clickable={props.clickable ? 'true' : 'false'}
      onClick={props.clickable ? props.onClick : undefined}
    >
      <span class="session-id">{props.session.id}</span>
      <span class="session-target">{props.session.target_name}</span>
      <span class="session-mode">{inputModeLabel(t, props.session.input_mode)}</span>
      <span class="session-meta">
        <Show when={isClosed() && duration()}>
          <span class="session-duration">{duration()}</span>
        </Show>
        <span class="session-reason" data-reason={reason().data}>
          {reason().label}
        </span>
      </span>
    </div>
  );
}
