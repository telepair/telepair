import { createResource, Show } from 'solid-js';
import { api, errorMessage } from '../lib/api';
import type { SystemInfo } from '../lib/protocol';
import { useI18n } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';
import AdminNav from '../components/AdminNav';
import Banner from '../components/Banner';

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const mins = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${mins}m`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

export default function AdminSystem() {
  const { t } = useI18n();

  const [info] = createResource<SystemInfo>(() => api.getSystemInfo());

  return (
    <div class="admin-system">
      <header class="topbar">
        <div class="topbar-left">
          <AdminNav current="/admin/system" />
        </div>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
        </div>
      </header>

      <main class="content">
        <h1>{t('admin_system.title')}</h1>
        <p class="subtitle">{t('admin_system.subtitle')}</p>

        <Show when={info.error}>
          <Banner variant="error">
            {t('admin_system.load_failed', { msg: errorMessage(info.error) })}
          </Banner>
        </Show>

        <Show when={info.loading}>
          <p class="muted">{t('admin_system.loading')}</p>
        </Show>

        <Show when={info()}>
          {(data) => (
            <div class="info-grid" data-testid="admin-system-grid">
              <div class="info-card">
                <span class="info-label">{t('admin_system.version')}</span>
                <span class="info-value">{data().version}</span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.uptime')}</span>
                <span class="info-value">{formatUptime(data().uptime_seconds)}</span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.live_sessions')}</span>
                <span class="info-value info-number">{data().live_sessions}</span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.registered_users')}</span>
                <span class="info-value info-number">{data().registered_users}</span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.data_dir')}</span>
                <span class="info-value info-path">{data().data_dir}</span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.db_path')}</span>
                <span class="info-value info-path">{data().db_path}</span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.targets_path')}</span>
                <span class="info-value info-path">
                  {data().targets_path ?? t('admin_system.not_configured')}
                </span>
              </div>
              <div class="info-card">
                <span class="info-label">{t('admin_system.smtp_status')}</span>
                <span
                  class="info-value"
                  data-status={data().smtp_configured ? 'ok' : 'off'}
                >
                  {data().smtp_configured
                    ? t('admin_system.smtp_configured')
                    : t('admin_system.smtp_not_configured')}
                </span>
              </div>
            </div>
          )}
        </Show>
      </main>

      <style>{`
        .admin-system { min-height: 100vh; }
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
        .content h1 {
          font-size: 18px;
          font-weight: 700;
          margin-bottom: 4px;
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
        .info-grid {
          display: grid;
          grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
          gap: 16px;
        }
        .info-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 8px;
          padding: 16px 18px;
          display: flex;
          flex-direction: column;
          gap: 6px;
        }
        .info-label {
          font-size: 12px;
          text-transform: uppercase;
          letter-spacing: 0.03em;
          color: var(--text-secondary);
        }
        .info-value {
          font-size: 15px;
          color: var(--text-primary);
          font-weight: 600;
        }
        .info-path {
          font-family: var(--font-mono);
          font-size: 13px;
          font-weight: 400;
          word-break: break-all;
        }
        .info-number {
          font-family: var(--font-mono);
          font-size: 22px;
        }
        .info-value[data-status='ok'] {
          color: var(--success);
        }
        .info-value[data-status='off'] {
          color: var(--warning);
        }
      `}</style>
    </div>
  );
}
