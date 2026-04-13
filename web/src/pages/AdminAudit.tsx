// web/src/pages/AdminAudit.tsx
import { createSignal, createResource, For, Show } from 'solid-js';
import { api, errorMessage } from '../lib/api';
import { AuditEventType } from '../lib/protocol';
import type { AuditEvent } from '../lib/protocol';
import type { ListAdminAuditOptions } from '../lib/api';
import { useI18n, type TranslationKey } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';
import Banner from '../components/Banner';

const PAGE_SIZE = 50;

/** Map dotted-lowercase event type → i18n key for human labels. */
const EVENT_LABEL_KEYS: Record<string, TranslationKey> = {
  [AuditEventType.SESSION_CREATED]: 'session_detail.event_session_created',
  [AuditEventType.SESSION_CLOSED]: 'session_detail.event_session_closed',
  [AuditEventType.PARTICIPANT_JOINED]: 'session_detail.event_participant_joined',
  [AuditEventType.INVITE_MINTED]: 'session_detail.event_invite_minted',
  [AuditEventType.INVITE_REDEEMED]: 'session_detail.event_invite_redeemed',
  [AuditEventType.INVITE_REVOKED]: 'session_detail.event_invite_revoked',
  [AuditEventType.TARGET_ACCESS_DENIED]: 'session_detail.event_target_access_denied',
  [AuditEventType.TARGET_RELOADED]: 'session_detail.event_target_reloaded',
  [AuditEventType.AUTH_LOGIN_FAILED]: 'session_detail.event_auth_login_failed',
  [AuditEventType.AUTH_REGISTER_REJECTED]: 'session_detail.event_auth_register_rejected',
  [AuditEventType.AUTH_REGISTER_COMPLETED]: 'session_detail.event_auth_register_completed',
  [AuditEventType.AUTH_VERIFY_FAILED]: 'session_detail.event_auth_verify_failed',
  [AuditEventType.AUTH_USER_ENABLED]: 'session_detail.event_auth_user_enabled',
  [AuditEventType.AUTH_USER_DISABLED]: 'session_detail.event_auth_user_disabled',
  [AuditEventType.AUTH_SESSION_ACCESS_DENIED]: 'session_detail.event_auth_session_access_denied',
  [AuditEventType.AUTH_PASSWORD_CHANGED]: 'session_detail.event_auth_password_changed',
  [AuditEventType.PARTICIPANT_ROLE_CHANGED]: 'session_detail.event_participant_role_changed',
};

/** All known event type values for the filter dropdown. */
const ALL_EVENT_TYPES = Object.values(AuditEventType);

function formatTs(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

export default function AdminAudit() {
  const { t } = useI18n();

  // Filter state
  const [filterType, setFilterType] = createSignal('');
  const [filterSession, setFilterSession] = createSignal('');

  // Applied filters (only update on explicit apply/clear)
  const [appliedType, setAppliedType] = createSignal('');
  const [appliedSession, setAppliedSession] = createSignal('');

  // Pagination
  const [offset, setOffset] = createSignal(0);
  const [hasMore, setHasMore] = createSignal(true);
  const [allRows, setAllRows] = createSignal<AuditEvent[]>([]);

  // Detail expansion
  const [expanded, setExpanded] = createSignal<Set<number>>(new Set());

  function buildOpts(): ListAdminAuditOptions {
    const opts: ListAdminAuditOptions = {
      limit: PAGE_SIZE,
      offset: offset(),
    };
    const et = appliedType();
    if (et) opts.event_type = et;
    const sid = appliedSession();
    if (sid) opts.session_id = sid;
    return opts;
  }

  const [page, { refetch }] = createResource(
    () => ({ offset: offset(), type: appliedType(), session: appliedSession() }),
    async () => {
      const rows = await api.listAdminAudit(buildOpts());
      setHasMore(rows.length >= PAGE_SIZE);
      if (offset() === 0) {
        setAllRows(rows);
      } else {
        setAllRows((prev) => [...prev, ...rows]);
      }
      return rows;
    },
  );

  function applyFilters() {
    setAppliedType(filterType());
    setAppliedSession(filterSession().trim());
    setOffset(0);
    setAllRows([]);
    setExpanded(new Set<number>());
  }

  function clearFilters() {
    setFilterType('');
    setFilterSession('');
    setAppliedType('');
    setAppliedSession('');
    setOffset(0);
    setAllRows([]);
    setExpanded(new Set<number>());
  }

  function loadMore() {
    setOffset(allRows().length);
  }

  function toggleExpanded(id: number) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function eventLabel(type: string): string {
    const key = EVENT_LABEL_KEYS[type];
    return key ? t(key) : type;
  }

  return (
    <div class="admin-audit">
      <header class="topbar">
        <div class="topbar-left">
          <a class="back-link" href="/">
            {t('admin_audit.back_to_dashboard')}
          </a>
          <h1>{t('admin_audit.title')}</h1>
        </div>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
        </div>
      </header>

      <main class="content">
        <p class="subtitle">{t('admin_audit.subtitle')}</p>

        {/* Filters */}
        <div class="filters">
          <div class="filter-group">
            <label for="filter-type">{t('admin_audit.filter_type_label')}</label>
            <select
              id="filter-type"
              value={filterType()}
              onChange={(e) => setFilterType(e.currentTarget.value)}
            >
              <option value="">{t('admin_audit.filter_type_all')}</option>
              <For each={ALL_EVENT_TYPES}>
                {(et) => <option value={et}>{eventLabel(et)}</option>}
              </For>
            </select>
          </div>
          <div class="filter-group">
            <label for="filter-session">{t('admin_audit.filter_session_label')}</label>
            <input
              id="filter-session"
              type="text"
              placeholder={t('admin_audit.filter_session_placeholder')}
              value={filterSession()}
              onInput={(e) => setFilterSession(e.currentTarget.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') applyFilters(); }}
            />
          </div>
          <div class="filter-actions">
            <button type="button" class="primary" onClick={applyFilters}>
              {t('admin_audit.filter_apply')}
            </button>
            <button type="button" onClick={clearFilters}>
              {t('admin_audit.filter_clear')}
            </button>
          </div>
        </div>

        <Show when={page.error}>
          <Banner variant="error">
            {t('admin_audit.load_failed', { msg: errorMessage(page.error) })}
          </Banner>
        </Show>

        <Show when={page.loading && allRows().length === 0}>
          <p class="muted">{t('admin_audit.loading')}</p>
        </Show>

        <Show when={!page.loading && allRows().length === 0 && !page.error}>
          <p class="muted">{t('admin_audit.empty')}</p>
        </Show>

        <Show when={allRows().length > 0}>
          <div class="audit-table-wrap" data-testid="admin-audit-table">
            <table class="audit-table">
              <thead>
                <tr>
                  <th>{t('admin_audit.col_time')}</th>
                  <th>{t('admin_audit.col_type')}</th>
                  <th>{t('admin_audit.col_actor')}</th>
                  <th>{t('admin_audit.col_session')}</th>
                  <th>{t('admin_audit.col_detail')}</th>
                </tr>
              </thead>
              <tbody>
                <For each={allRows()}>
                  {(row) => {
                    const id = row.id ?? -1;
                    return (
                      <>
                        <tr>
                          <td class="col-time">{formatTs(row.ts)}</td>
                          <td>
                            <span class="event-badge">{eventLabel(row.event_type)}</span>
                          </td>
                          <td class="col-actor">
                            {row.actor_name ?? t('admin_audit.no_actor')}
                          </td>
                          <td class="col-session">
                            <Show when={row.session_id} fallback={t('admin_audit.no_session')}>
                              <span class="session-id">{row.session_id}</span>
                            </Show>
                          </td>
                          <td>
                            <Show when={row.detail != null}>
                              <button
                                type="button"
                                class="detail-toggle"
                                onClick={() => toggleExpanded(id)}
                                aria-label={t('admin_audit.detail_toggle')}
                              >
                                {expanded().has(id) ? '▼' : '▶'}
                              </button>
                            </Show>
                          </td>
                        </tr>
                        <Show when={expanded().has(id) && row.detail != null}>
                          <tr class="detail-row">
                            <td colspan="5">
                              <pre class="detail-json">
                                {JSON.stringify(row.detail, null, 2)}
                              </pre>
                            </td>
                          </tr>
                        </Show>
                      </>
                    );
                  }}
                </For>
              </tbody>
            </table>
          </div>

          <div class="pagination">
            <Show
              when={hasMore()}
              fallback={<span class="muted">{t('admin_audit.no_more')}</span>}
            >
              <button
                type="button"
                onClick={loadMore}
                disabled={page.loading}
              >
                {page.loading ? t('admin_audit.loading') : t('admin_audit.load_more')}
              </button>
            </Show>
          </div>
        </Show>
      </main>

      <style>{`
        .admin-audit { min-height: 100vh; }
        .topbar {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 12px 24px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
          gap: 16px;
        }
        .topbar-left {
          display: flex;
          align-items: center;
          gap: 16px;
          min-width: 0;
        }
        .topbar h1 {
          font-size: 18px;
          font-weight: 700;
          white-space: nowrap;
        }
        .back-link {
          color: var(--text-secondary);
          text-decoration: none;
          font-size: 13px;
        }
        .back-link:hover { color: var(--text-primary); }
        .topbar-actions { display: flex; gap: 8px; align-items: center; }
        .content {
          padding: 24px;
          max-width: 1100px;
          margin: 0 auto;
        }
        .subtitle {
          color: var(--text-secondary);
          font-size: 13px;
          margin-bottom: 24px;
          line-height: 1.6;
        }
        .muted {
          color: var(--text-secondary);
          font-size: 14px;
        }

        /* Filters */
        .filters {
          display: flex;
          align-items: flex-end;
          gap: 16px;
          margin-bottom: 20px;
          flex-wrap: wrap;
        }
        .filter-group {
          display: flex;
          flex-direction: column;
          gap: 4px;
        }
        .filter-group label {
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
        }
        .filter-group select,
        .filter-group input {
          font-size: 13px;
          padding: 6px 10px;
          border: 1px solid var(--border);
          border-radius: 6px;
          background: var(--bg-primary);
          color: var(--text-primary);
          min-width: 180px;
        }
        .filter-actions {
          display: flex;
          gap: 8px;
          align-items: center;
        }
        .filter-actions button {
          font-size: 13px;
          padding: 6px 14px;
          border-radius: 6px;
          cursor: pointer;
        }

        /* Table */
        .audit-table-wrap {
          overflow-x: auto;
          border: 1px solid var(--border);
          border-radius: 8px;
        }
        .audit-table {
          width: 100%;
          border-collapse: collapse;
          font-size: 13px;
        }
        .audit-table th {
          text-align: left;
          padding: 10px 14px;
          font-size: 12px;
          text-transform: uppercase;
          letter-spacing: 0.03em;
          color: var(--text-secondary);
          background: var(--bg-secondary);
          border-bottom: 1px solid var(--border);
        }
        .audit-table td {
          padding: 8px 14px;
          border-bottom: 1px solid var(--border);
          color: var(--text-primary);
          vertical-align: top;
        }
        .audit-table tr:last-child td {
          border-bottom: none;
        }
        .col-time {
          white-space: nowrap;
          font-family: var(--font-mono);
          font-size: 12px;
          color: var(--text-secondary);
        }
        .col-actor {
          max-width: 160px;
          overflow: hidden;
          text-overflow: ellipsis;
          white-space: nowrap;
        }
        .col-session {
          max-width: 120px;
          overflow: hidden;
          text-overflow: ellipsis;
        }
        .session-id {
          font-family: var(--font-mono);
          font-size: 12px;
          color: var(--text-secondary);
        }
        .event-badge {
          font-size: 12px;
          padding: 2px 8px;
          border-radius: 999px;
          background: var(--bg-tertiary);
          color: var(--text-secondary);
          white-space: nowrap;
        }
        .detail-toggle {
          background: none;
          border: none;
          cursor: pointer;
          font-size: 11px;
          color: var(--text-secondary);
          padding: 2px 6px;
        }
        .detail-toggle:hover {
          color: var(--text-primary);
        }
        .detail-row td {
          padding: 0 14px 10px 14px;
          border-bottom: 1px solid var(--border);
        }
        .detail-json {
          margin: 0;
          padding: 10px;
          font-size: 12px;
          font-family: var(--font-mono);
          background: var(--bg-tertiary);
          border-radius: 6px;
          overflow-x: auto;
          white-space: pre-wrap;
          word-break: break-all;
        }

        /* Pagination */
        .pagination {
          display: flex;
          justify-content: center;
          padding: 16px 0;
        }
        .pagination button {
          font-size: 13px;
          padding: 6px 20px;
          border-radius: 6px;
          cursor: pointer;
        }
        .pagination button:disabled {
          opacity: 0.6;
          cursor: default;
        }
      `}</style>
    </div>
  );
}
