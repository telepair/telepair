// web/src/pages/AdminUsers.tsx
import { createResource, createSignal, For, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { api, errorMessage } from '../lib/api';
import type { AdminUserInfo } from '../lib/protocol';
import { toast } from '../stores/toast';
import { auth } from '../stores/auth';
import { useI18n } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';
import Banner from '../components/Banner';

export default function AdminUsers() {
  const { t } = useI18n();
  const navigate = useNavigate();

  const [users, { refetch }] = createResource<AdminUserInfo[]>(() =>
    api.listAdminUsers(),
  );
  const [busyId, setBusyId] = createSignal<string | null>(null);

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
          <a class="back-link" href="/">
            {t('admin_users.back_to_dashboard')}
          </a>
          <h1>{t('admin_users.title')}</h1>
        </div>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
        </div>
      </header>

      <main class="content">
        <p class="subtitle">{t('admin_users.subtitle')}</p>

        <Show when={users.error}>
          <Banner variant="error">
            {t('admin_users.load_failed', { msg: errorMessage(users.error) })}
          </Banner>
        </Show>

        <Show when={users.loading}>
          <p class="muted">{t('admin_users.loading')}</p>
        </Show>

        <Show when={!users.loading && users()?.length === 0}>
          <p class="muted">{t('admin_users.empty')}</p>
        </Show>

        <Show when={(users() ?? []).length > 0}>
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
                <For each={users()}>
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
                          data-enabled={user.session_enabled ? 'true' : 'false'}
                        >
                          {user.session_enabled
                            ? t('admin_users.sessions_enabled')
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
        .session-badge[data-enabled='true'] {
          background: rgba(63, 185, 80, 0.15);
          color: var(--success);
        }
        .session-badge[data-enabled='false'] {
          background: rgba(210, 153, 34, 0.15);
          color: var(--warning);
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
      `}</style>
    </div>
  );
}
