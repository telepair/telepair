// web/src/pages/AdminUsers.tsx
import { createSignal, createResource, For, Show } from 'solid-js';
import { api, errorMessage } from '../lib/api';
import type { AdminUserInfo } from '../lib/protocol';
import { toast } from '../stores/toast';
import { auth } from '../stores/auth';
import { useI18n } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';
import AdminNav from '../components/AdminNav';
import Banner from '../components/Banner';

const PAGE_SIZE = 50;

export default function AdminUsers() {
  const { t } = useI18n();

  // Filter state (input)
  const [filterQuery, setFilterQuery] = createSignal('');
  const [filterStatus, setFilterStatus] = createSignal('');

  // Applied filters
  const [appliedQuery, setAppliedQuery] = createSignal('');
  const [appliedStatus, setAppliedStatus] = createSignal('');

  // Pagination
  const [offset, setOffset] = createSignal(0);
  const [hasMore, setHasMore] = createSignal(true);
  const [allRows, setAllRows] = createSignal<AdminUserInfo[]>([]);

  const [busyId, setBusyId] = createSignal<string | null>(null);

  const [page, { refetch }] = createResource(
    () => ({ offset: offset(), q: appliedQuery(), status: appliedStatus() }),
    async () => {
      const resp = await api.listAdminUsers({
        q: appliedQuery() || undefined,
        status: appliedStatus() || undefined,
        limit: PAGE_SIZE,
        offset: offset(),
      });
      setHasMore(resp.users.length >= PAGE_SIZE);
      if (offset() === 0) {
        setAllRows(resp.users);
      } else {
        setAllRows((prev) => [...prev, ...resp.users]);
      }
      return resp;
    },
  );

  function applyFilters() {
    setAppliedQuery(filterQuery().trim());
    setAppliedStatus(filterStatus());
    setOffset(0);
    setAllRows([]);
  }

  function clearFilters() {
    setFilterQuery('');
    setFilterStatus('');
    setAppliedQuery('');
    setAppliedStatus('');
    setOffset(0);
    setAllRows([]);
  }

  function loadMore() {
    setOffset(allRows().length);
  }

  const handleToggle = async (user: AdminUserInfo) => {
    if (busyId()) return;
    setBusyId(user.id);
    try {
      if (user.session_enabled) {
        await api.disableAdminUser(user.id);
        toast.success(t('admin_users.disable_success', { name: user.name }), {
          id: 'admin-users-toggle',
        });
      } else {
        await api.enableAdminUser(user.id);
        toast.success(t('admin_users.enable_success', { name: user.name }), {
          id: 'admin-users-toggle',
        });
      }
      // Re-fetch current page to reflect the change
      setOffset(0);
      setAllRows([]);
      await refetch();
    } catch (e) {
      toast.error(t('admin_users.action_failed', { msg: errorMessage(e) }), {
        id: 'admin-users-toggle',
      });
    } finally {
      setBusyId(null);
    }
  };

  const isSelf = (user: AdminUserInfo) => user.id === auth.currentUserId();

  return (
    <div class="admin-users">
      <header class="topbar">
        <div class="topbar-left">
          <AdminNav current="/admin/users" />
        </div>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
        </div>
      </header>

      <main class="content">
        <p class="subtitle">{t('admin_users.subtitle')}</p>

        {/* Filters */}
        <div class="filters">
          <div class="filter-group">
            <label for="filter-query">{t('admin_users.filter_query_label')}</label>
            <input
              id="filter-query"
              type="text"
              placeholder={t('admin_users.filter_query_placeholder')}
              value={filterQuery()}
              onInput={(e) => setFilterQuery(e.currentTarget.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') applyFilters(); }}
            />
          </div>
          <div class="filter-group">
            <label for="filter-status">{t('admin_users.filter_status_label')}</label>
            <select
              id="filter-status"
              value={filterStatus()}
              onChange={(e) => setFilterStatus(e.currentTarget.value)}
            >
              <option value="">{t('admin_users.filter_status_all')}</option>
              <option value="enabled">{t('admin_users.filter_status_enabled')}</option>
              <option value="disabled">{t('admin_users.filter_status_disabled')}</option>
              <option value="pending">{t('admin_users.filter_status_pending')}</option>
            </select>
          </div>
          <div class="filter-actions">
            <button type="button" class="primary" onClick={applyFilters}>
              {t('admin_users.filter_apply')}
            </button>
            <button type="button" onClick={clearFilters}>
              {t('admin_users.filter_clear')}
            </button>
          </div>
        </div>

        <Show when={page.error}>
          <Banner variant="error">
            {t('admin_users.load_failed', { msg: errorMessage(page.error) })}
          </Banner>
        </Show>

        <Show when={page.loading && allRows().length === 0}>
          <p class="muted">{t('admin_users.loading')}</p>
        </Show>

        <Show when={!page.loading && allRows().length === 0 && !page.error}>
          <p class="muted">
            {appliedQuery() || appliedStatus()
              ? t('admin_users.no_results')
              : t('admin_users.empty')}
          </p>
        </Show>

        <Show when={allRows().length > 0}>
          <div class="user-table-wrap" data-testid="admin-users-table">
            <table class="user-table">
              <thead>
                <tr>
                  <th>{t('admin_users.col_name')}</th>
                  <th>{t('admin_users.col_email')}</th>
                  <th>{t('admin_users.col_role')}</th>
                  <th>{t('admin_users.col_sessions')}</th>
                  <th>{t('admin_users.col_actions')}</th>
                </tr>
              </thead>
              <tbody>
                <For each={allRows()}>
                  {(user) => (
                    <tr data-user-id={user.id}>
                      <td class="user-name">
                        {user.name}
                        <Show when={isSelf(user)}>
                          <span class="self-badge">{t('admin_users.self_label')}</span>
                        </Show>
                      </td>
                      <td class="user-email">{user.email ?? '—'}</td>
                      <td>
                        <span
                          class="role-badge"
                          data-admin={user.is_admin ? 'true' : 'false'}
                        >
                          {user.is_admin
                            ? t('admin_users.role_admin')
                            : t('admin_users.role_user')}
                        </span>
                      </td>
                      <td>
                        <span
                          class="session-badge"
                          data-state={
                            user.session_enabled
                              ? 'enabled'
                              : user.approval_state === 'pending'
                                ? 'pending'
                                : 'disabled'
                          }
                        >
                          {user.session_enabled
                            ? t('admin_users.sessions_enabled')
                            : user.approval_state === 'pending'
                              ? t('admin_users.sessions_pending')
                              : t('admin_users.sessions_disabled')}
                        </span>
                      </td>
                      <td>
                        <Show
                          when={!isSelf(user)}
                          fallback={<span class="muted">—</span>}
                        >
                          <button
                            type="button"
                            class="toggle-btn"
                            data-action={user.session_enabled ? 'disable' : 'enable'}
                            disabled={busyId() === user.id}
                            onClick={() => handleToggle(user)}
                          >
                            {busyId() === user.id
                              ? user.session_enabled
                                ? t('admin_users.action_disabling')
                                : t('admin_users.action_enabling')
                              : user.session_enabled
                                ? t('admin_users.action_disable')
                                : t('admin_users.action_enable')}
                          </button>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>

          <div class="pagination">
            <Show
              when={hasMore()}
              fallback={<span class="muted">{t('admin_users.no_more')}</span>}
            >
              <button
                type="button"
                onClick={loadMore}
                disabled={page.loading}
              >
                {page.loading ? t('admin_users.loading') : t('admin_users.load_more')}
              </button>
            </Show>
          </div>
        </Show>
      </main>

      <style>{`
        .admin-users { min-height: 100vh; }
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
        .topbar-actions { display: flex; gap: 8px; align-items: center; }
        .content {
          padding: 24px;
          max-width: 960px;
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
        .user-table-wrap {
          overflow-x: auto;
          border: 1px solid var(--border);
          border-radius: 8px;
        }
        .user-table {
          width: 100%;
          border-collapse: collapse;
          font-size: 14px;
        }
        .user-table th {
          text-align: left;
          padding: 10px 14px;
          font-size: 12px;
          text-transform: uppercase;
          letter-spacing: 0.03em;
          color: var(--text-secondary);
          background: var(--bg-secondary);
          border-bottom: 1px solid var(--border);
        }
        .user-table td {
          padding: 10px 14px;
          border-bottom: 1px solid var(--border);
          color: var(--text-primary);
        }
        .user-table tr:last-child td {
          border-bottom: none;
        }
        .user-name {
          font-weight: 600;
          display: flex;
          align-items: center;
          gap: 6px;
        }
        .self-badge {
          font-size: 11px;
          color: var(--text-secondary);
          font-weight: 400;
        }
        .user-email {
          font-family: var(--font-mono);
          font-size: 12px;
          color: var(--text-secondary);
        }
        .role-badge {
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 999px;
          background: var(--bg-tertiary);
          color: var(--text-secondary);
        }
        .role-badge[data-admin='true'] {
          background: #5b2b2b;
          color: #ffd;
        }
        .session-badge {
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 999px;
        }
        .session-badge[data-state='enabled'] {
          background: rgba(63, 185, 80, 0.15);
          color: var(--success);
        }
        .session-badge[data-state='disabled'] {
          background: rgba(210, 153, 34, 0.15);
          color: var(--warning);
        }
        .session-badge[data-state='pending'] {
          background: rgba(88, 166, 255, 0.15);
          color: var(--accent, #58a6ff);
        }
        .toggle-btn {
          font-size: 12px;
          padding: 4px 12px;
          border-radius: 6px;
          cursor: pointer;
          transition: all 0.15s;
        }
        .toggle-btn[data-action='enable'] {
          border-color: var(--success);
          color: var(--success);
        }
        .toggle-btn[data-action='enable']:hover {
          background: rgba(63, 185, 80, 0.15);
        }
        .toggle-btn[data-action='disable'] {
          border-color: var(--warning);
          color: var(--warning);
        }
        .toggle-btn[data-action='disable']:hover {
          background: rgba(210, 153, 34, 0.15);
        }
        .toggle-btn:disabled {
          opacity: 0.6;
          cursor: default;
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
