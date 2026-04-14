import { useI18n, type TranslationKey } from '../i18n';

interface AdminNavProps {
  current: string;
}

const LINKS: { href: string; key: TranslationKey }[] = [
  { href: '/', key: 'admin_nav.dashboard' },
  { href: '/admin/system', key: 'admin_nav.system' },
  { href: '/admin/users', key: 'admin_nav.users' },
  { href: '/admin/targets', key: 'admin_nav.targets' },
  { href: '/admin/audit', key: 'admin_nav.audit' },
];

export default function AdminNav(props: AdminNavProps) {
  const { t } = useI18n();

  return (
    <nav class="admin-nav" aria-label="Admin navigation">
      {LINKS.map((link) => (
        <a
          class="admin-nav-link"
          href={link.href}
          data-active={props.current === link.href ? 'true' : 'false'}
        >
          {t(link.key)}
        </a>
      ))}

      <style>{`
        .admin-nav {
          display: flex;
          gap: 4px;
          align-items: center;
        }
        .admin-nav-link {
          padding: 4px 12px;
          border-radius: 6px;
          font-size: 13px;
          color: var(--text-secondary);
          text-decoration: none;
          transition: all 0.15s;
        }
        .admin-nav-link:hover {
          color: var(--text-primary);
          background: var(--bg-tertiary);
        }
        .admin-nav-link[data-active='true'] {
          color: var(--text-primary);
          background: var(--bg-tertiary);
          font-weight: 600;
        }
      `}</style>
    </nav>
  );
}
