// web/src/pages/AdminTargets.tsx
//
// Admin-only target management page. Rendered inside `AdminGuard` so
// by the time this component mounts we already know `auth.currentUserIsAdmin()`
// is `true` — we do NOT re-check it here, and a stray render on a
// non-admin tab would surface as the standard 403 banner from the
// failed list request (safer than silently showing an empty page).
//
// Contract:
//   - `GET /api/admin/targets`         — full detail + redacted env
//   - `POST /api/admin/targets/reload` — atomic hot-reload
// Both are admin-only on the backend. Any 4xx here is treated as a
// hard error (shown in a banner) rather than a silent empty state.
import { createResource, createSignal, For, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { api, ApiError, errorMessage } from '../lib/api';
import type { AdminTargetInfo, ValidateTargetsResult } from '../lib/protocol';
import { toast } from '../stores/toast';
import { useI18n, type Translator } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';
import AdminNav from '../components/AdminNav';
import Banner from '../components/Banner';
import ReloadConfirmDialog from '../components/ReloadConfirmDialog';

/**
 * One row of the `still_referenced` payload: a target name and the
 * count of live sessions still pointing at it. Exposed as a named
 * type so the banner renderer can iterate it without re-deriving
 * the shape.
 */
export interface BlockingTarget {
  target: string;
  active_sessions: number;
}

/**
 * Parsed shape of the structured JSON body that `reloadTargets`
 * returns on 4xx. `targets` is only populated for the
 * `still_referenced` reason — the backend's other reasons
 * (`no_targets_path`, `parse_error`) leave it undefined. Exported so
 * the unit tests in `AdminTargets.parseReloadError.test.ts` can
 * assert the parser's contract without spinning up the page.
 */
export interface ParsedReloadError {
  reason: string;
  message: string;
  targets?: BlockingTarget[];
}

/**
 * Parse the structured JSON body that `reloadTargets` returns on 4xx
 * into a typed shape. The backend intentionally uses distinct
 * `reason` codes (`no_targets_path`, `parse_error`,
 * `still_referenced`) so the UI can pick the right translated
 * message instead of dumping the raw server string. `null` means
 * the body wasn't JSON we understand — the caller then falls back
 * to a generic toast carrying the unparsed text.
 *
 * Exported (rather than module-private) so a sibling unit test can
 * pin the parser independently of the page render — the
 * `still_referenced` shape is the load-bearing part of the admin
 * reload guard's UX, and a future server change that renames a
 * field would otherwise only blow up at integration time.
 */
export function parseReloadError(raw: string): ParsedReloadError | null {
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return null;
    const reason = typeof parsed.reason === 'string' ? parsed.reason : '';
    if (!reason) return null;
    const message = typeof parsed.message === 'string' ? parsed.message : '';
    // `targets` is structurally validated here so downstream
    // renderers can iterate it without re-checking each row. A
    // malformed entry (missing fields, wrong types) is dropped
    // silently — the banner falls back to "no rows", which is
    // strictly better than a runtime crash inside `<For>`.
    let targets: BlockingTarget[] | undefined;
    if (Array.isArray(parsed.targets)) {
      targets = parsed.targets
        .filter(
          (row: unknown): row is BlockingTarget =>
            !!row &&
            typeof row === 'object' &&
            typeof (row as BlockingTarget).target === 'string' &&
            typeof (row as BlockingTarget).active_sessions === 'number',
        )
        .map((row: BlockingTarget) => ({
          target: row.target,
          active_sessions: row.active_sessions,
        }));
    }
    return { reason, message, targets };
  } catch {
    // Fall through to `null` — caller handles it.
    return null;
  }
}

/** Map a `type` field to a short localized label. Unknown kinds are
 *  passed through verbatim so a forward-compat server addition is
 *  visible to the admin rather than hidden behind a blank badge. */
function kindLabel(kind: string, t: Translator): string {
  if (kind === 'virtual') return t('admin_targets.kind_virtual');
  if (kind === 'local') return t('admin_targets.kind_local');
  return kind;
}

/** Pluralize the active-session counter with a graceful zero state. */
function activeSessionsLabel(n: number, t: Translator): string {
  if (n === 0) return t('admin_targets.active_sessions_zero');
  if (n === 1) return t('admin_targets.active_sessions_singular', { n: '1' });
  return t('admin_targets.active_sessions_plural', { n: String(n) });
}

export default function AdminTargets() {
  const { t } = useI18n();
  const navigate = useNavigate();

  // `createResource` gives us refetch + pending state out of the box.
  // The fetcher throws on any non-2xx — the `<Show>` below renders
  // the error banner when `targets.error` is set.
  const [targets, { refetch }] = createResource<AdminTargetInfo[]>(() =>
    api.listAdminTargets(),
  );
  const [reloading, setReloading] = createSignal(false);
  // `still_referenced` is the only reload error that needs more than
  // a single-line toast: it ships a structured list of targets that
  // are still in use, and the admin needs to see all of them at once
  // to decide which sessions to close. A toast (transient, single
  // line) actively hides that information; a persistent banner
  // pinned above the grid does not. Cleared at the start of every
  // reload attempt so a previously-blocked page doesn't keep
  // showing a stale list after the admin successfully reloaded.
  const [blockingTargets, setBlockingTargets] = createSignal<
    BlockingTarget[] | null
  >(null);
  const [validateResult, setValidateResult] = createSignal<ValidateTargetsResult | null>(null);

  const handleReloadClick = async () => {
    if (reloading()) return;
    setReloading(true);
    setBlockingTargets(null);
    try {
      const result = await api.validateTargets();
      if (!result.valid) {
        toast.error(
          t('admin_targets.validate_error', {
            msg: result.errors?.join('; ') ?? 'Unknown error',
          }),
          { id: 'admin-targets-reload' },
        );
        return;
      }

      const diff = result.diff;
      const hasChanges =
        diff &&
        (diff.added.length > 0 || diff.removed.length > 0 || diff.changed.length > 0);

      if (!hasChanges) {
        // No changes — still pin the reload to the sha we just
        // previewed; if the file changes before this call hits the
        // server, the 409 path below surfaces the race to the admin.
        await api.reloadTargets(result.expected_sha256);
        toast.success(t('admin_targets.validate_no_changes'), {
          id: 'admin-targets-reload',
        });
        await refetch();
        return;
      }

      // Show confirmation dialog
      setValidateResult(result);
    } catch (e) {
      toast.error(
        t('admin_targets.reload_failed_generic', { msg: errorMessage(e) }),
        { id: 'admin-targets-reload' },
      );
    } finally {
      setReloading(false);
    }
  };

  const handleConfirmReload = async () => {
    setReloading(true);
    const expectedSha = validateResult()?.expected_sha256;
    try {
      const result = await api.reloadTargets(expectedSha);
      toast.success(
        t('admin_targets.reload_success', {
          count: String(result.targets),
          path: result.path,
        }),
        { id: 'admin-targets-reload' },
      );
      setValidateResult(null);
      await refetch();
    } catch (e) {
      if (e instanceof ApiError) {
        const parsed = parseReloadError(e.message);
        if (parsed?.reason === 'still_referenced' && parsed.targets) {
          setBlockingTargets(parsed.targets);
          setValidateResult(null);
        } else if (parsed?.reason === 'file_changed') {
          // Another writer touched targets.yaml between validate and
          // confirm. Drop the dialog and force the admin to re-preview
          // so what they approve matches what the server would apply.
          setValidateResult(null);
          toast.error(t('admin_targets.reload_file_changed'), {
            id: 'admin-targets-reload',
          });
        } else {
          toast.error(
            t('admin_targets.reload_failed_generic', { msg: e.message }),
            { id: 'admin-targets-reload' },
          );
        }
      } else {
        toast.error(
          t('admin_targets.reload_failed_generic', { msg: errorMessage(e) }),
          { id: 'admin-targets-reload' },
        );
      }
    } finally {
      setReloading(false);
    }
  };

  const handleActiveSessionsClick = (name: string) => {
    // Deep link into the Dashboard filter. The dashboard's
    // createEffect picks this up and issues the filtered fetch.
    navigate(`/?target=${encodeURIComponent(name)}&status=active`);
  };

  return (
    <div class="admin-targets">
      <header class="topbar">
        <div class="topbar-left">
          <AdminNav current="/admin/targets" />
        </div>
        <div class="topbar-actions">
          <LocaleSwitcher variant="topbar" />
          <button
            class="reload-btn"
            onClick={handleReloadClick}
            disabled={reloading()}
            data-testid="admin-targets-reload-button"
          >
            {reloading() ? t('admin_targets.reloading') : t('admin_targets.reload')}
          </button>
        </div>
      </header>

      <main class="content">
        <p class="subtitle">{t('admin_targets.subtitle')}</p>

        <Show when={targets.error}>
          {/*
            `targets.error` will be an `ApiError` with the raw body
            as its message when the list call fails. 401 is already
            handled upstream (the api layer's interceptor bounces to
            /login); we only see 403 / 500 / network here — all of
            which deserve a loud banner so the admin knows the page
            is in a broken state.
          */}
          <Banner variant="error">
            {t('admin_targets.load_failed', { msg: errorMessage(targets.error) })}
          </Banner>
        </Show>

        <Show when={blockingTargets()}>
          {/*
            Reload guard's structured rejection: the new yaml would
            drop targets that still have live PTYs. We render the
            full list (target name + active session count) so the
            admin can decide which sessions to close before
            retrying. Persistent banner, not a toast — the data
            density makes "auto-dismiss after 5s" actively hostile.
          */}
          <Banner variant="error">
            <div
              class="reload-blocked"
              data-testid="admin-targets-reload-blocked"
            >
              <p class="reload-blocked-title">
                {t('admin_targets.reload_failed_still_referenced_title')}
              </p>
              <ul class="reload-blocked-list">
                <For each={blockingTargets() ?? []}>
                  {(row) => (
                    <li data-testid={`admin-targets-blocked-row-${row.target}`}>
                      {t('admin_targets.reload_failed_still_referenced_row', {
                        target: row.target,
                        count: String(row.active_sessions),
                      })}
                    </li>
                  )}
                </For>
              </ul>
              <p class="reload-blocked-hint">
                {t('admin_targets.reload_failed_still_referenced_hint')}
              </p>
            </div>
          </Banner>
        </Show>

        <Show when={targets.loading}>
          <p class="muted">{t('admin_targets.loading')}</p>
        </Show>

        <Show when={!targets.loading && targets()?.length === 0}>
          <p class="muted">{t('admin_targets.empty')}</p>
        </Show>

        <Show when={(targets() ?? []).length > 0}>
          <div class="target-grid" data-testid="admin-targets-grid">
            <For each={targets()}>
              {(target) => (
                <TargetCard
                  target={target}
                  onActiveSessionsClick={() =>
                    handleActiveSessionsClick(target.name)
                  }
                  t={t}
                />
              )}
            </For>
          </div>
        </Show>
      </main>

      <ReloadConfirmDialog
        result={validateResult()}
        reloading={reloading()}
        onConfirm={handleConfirmReload}
        onCancel={() => setValidateResult(null)}
      />

      <style>{`
        .admin-targets { min-height: 100vh; }
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
        .reload-btn:disabled {
          opacity: 0.6;
          cursor: default;
        }
        .content {
          padding: 24px;
          max-width: 1080px;
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
          padding: 16px 0;
        }
        .target-grid {
          display: grid;
          grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
          gap: 16px;
        }
        .target-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 8px;
          padding: 16px 18px;
          display: flex;
          flex-direction: column;
          gap: 10px;
        }
        .target-head {
          display: flex;
          align-items: flex-start;
          justify-content: space-between;
          gap: 8px;
        }
        .target-title-block { min-width: 0; }
        .target-display {
          font-weight: 600;
          color: var(--text-primary);
          margin: 0 0 2px 0;
          line-height: 1.3;
          word-break: break-word;
        }
        .target-id {
          font-family: var(--font-mono);
          font-size: 12px;
          color: var(--text-secondary);
        }
        .target-badges {
          display: flex;
          gap: 6px;
          flex-wrap: wrap;
          align-items: center;
        }
        .badge {
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 999px;
          background: var(--bg-tertiary);
          color: var(--text-secondary);
          white-space: nowrap;
        }
        .badge-admin {
          background: #5b2b2b;
          color: #ffd;
        }
        .target-row {
          display: flex;
          gap: 8px;
          font-size: 13px;
          align-items: baseline;
        }
        .target-label {
          color: var(--text-secondary);
          min-width: 72px;
          font-size: 12px;
          text-transform: uppercase;
          letter-spacing: 0.03em;
        }
        .target-value {
          color: var(--text-primary);
          font-family: var(--font-mono);
          word-break: break-all;
          flex: 1;
        }
        .target-args {
          display: flex;
          flex-wrap: wrap;
          gap: 4px;
          flex: 1;
        }
        .arg-chip {
          font-family: var(--font-mono);
          font-size: 11px;
          padding: 1px 6px;
          background: var(--bg-tertiary);
          border-radius: 4px;
          color: var(--text-primary);
        }
        .tags {
          display: flex;
          flex-wrap: wrap;
          gap: 4px;
          flex: 1;
        }
        .tag {
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 12px;
          background: var(--bg-tertiary);
          color: var(--text-secondary);
        }
        .env-list {
          display: flex;
          flex-wrap: wrap;
          gap: 4px;
          flex: 1;
        }
        .env-chip {
          font-family: var(--font-mono);
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 4px;
          border: 1px solid var(--border);
          display: inline-flex;
          gap: 4px;
          align-items: center;
        }
        .env-chip[data-set='true'] {
          background: var(--bg-tertiary);
          color: var(--text-primary);
        }
        .env-chip[data-set='false'] {
          background: transparent;
          color: var(--text-secondary);
          border-style: dashed;
        }
        .env-chip-state {
          font-size: 10px;
          color: var(--text-secondary);
        }
        .target-footer {
          display: flex;
          align-items: center;
          justify-content: flex-end;
          padding-top: 6px;
          border-top: 1px dashed var(--border);
          margin-top: 4px;
        }
        .sessions-link {
          background: transparent;
          border: 1px solid var(--border);
          color: var(--text-secondary);
          padding: 6px 12px;
          border-radius: 999px;
          font: inherit;
          font-size: 12px;
          cursor: pointer;
          transition: all 0.15s;
        }
        .sessions-link[data-active='true'] {
          border-color: var(--accent);
          color: var(--text-primary);
        }
        .sessions-link[data-active='false'] {
          cursor: default;
        }
        .sessions-link[data-active='true']:hover {
          background: var(--accent);
          color: var(--bg-primary);
        }
        .reload-blocked {
          display: flex;
          flex-direction: column;
          gap: 8px;
        }
        .reload-blocked-title {
          font-weight: 600;
          margin: 0;
        }
        .reload-blocked-list {
          margin: 0;
          padding-left: 18px;
          display: flex;
          flex-direction: column;
          gap: 2px;
        }
        .reload-blocked-list li {
          font-family: var(--font-mono);
          font-size: 12px;
        }
        .reload-blocked-hint {
          margin: 0;
          font-size: 12px;
          opacity: 0.85;
        }
      `}</style>
    </div>
  );
}

// --- Card component -----------------------------------------------------

interface TargetCardProps {
  target: AdminTargetInfo;
  onActiveSessionsClick: () => void;
  t: Translator;
}

function TargetCard(props: TargetCardProps) {
  const hasActive = () => props.target.active_sessions > 0;
  return (
    <div class="target-card" data-target-name={props.target.name}>
      <div class="target-head">
        <div class="target-title-block">
          <p class="target-display">{props.target.display}</p>
          <span class="target-id">{props.target.name}</span>
        </div>
        <div class="target-badges">
          <span class="badge">{kindLabel(props.target.type, props.t)}</span>
          <Show when={props.target.admin_only}>
            <span class="badge badge-admin">
              {props.t('admin_targets.admin_only_badge')}
            </span>
          </Show>
        </div>
      </div>

      <Show when={props.target.command}>
        <div class="target-row">
          <span class="target-label">{props.t('admin_targets.command_label')}</span>
          <span class="target-value">{props.target.command}</span>
        </div>
      </Show>

      <Show when={props.target.shell}>
        <div class="target-row">
          <span class="target-label">{props.t('admin_targets.shell_label')}</span>
          <span class="target-value">{props.target.shell}</span>
        </div>
      </Show>

      <Show when={props.target.args.length > 0}>
        <div class="target-row">
          <span class="target-label">
            {props.t('admin_targets.args_count', {
              n: String(props.target.args.length),
            })}
          </span>
          <div class="target-args">
            <For each={props.target.args}>
              {(arg) => <span class="arg-chip">{arg}</span>}
            </For>
          </div>
        </div>
      </Show>

      <Show when={props.target.tags.length > 0}>
        <div class="target-row">
          <span class="target-label">{props.t('admin_targets.tags_label')}</span>
          <div class="tags">
            <For each={props.target.tags}>
              {(tag) => <span class="tag">{tag}</span>}
            </For>
          </div>
        </div>
      </Show>

      <div class="target-row">
        <span class="target-label">{props.t('admin_targets.env_label')}</span>
        <Show
          when={props.target.env.length > 0}
          fallback={<span class="target-value">{props.t('admin_targets.env_empty')}</span>}
        >
          <div class="env-list">
            <For each={props.target.env}>
              {(entry) => (
                <span class="env-chip" data-set={entry.set ? 'true' : 'false'}>
                  {entry.key}
                  <span class="env-chip-state">
                    {entry.set
                      ? props.t('admin_targets.env_set')
                      : props.t('admin_targets.env_unset')}
                  </span>
                </span>
              )}
            </For>
          </div>
        </Show>
      </div>

      <div class="target-footer">
        {/*
          `sessions-link` is a real button even when the count is
          zero — it stays in the layout so the cards have consistent
          vertical rhythm, but it's inert (no hover, no click
          handler) in that state. When the count is non-zero it
          deep-links into the Dashboard sessions tab filtered by
          this target.
        */}
        <button
          type="button"
          class="sessions-link"
          data-active={hasActive() ? 'true' : 'false'}
          data-testid={`admin-targets-sessions-link-${props.target.name}`}
          onClick={hasActive() ? props.onActiveSessionsClick : undefined}
          disabled={!hasActive()}
          aria-label={props.t('admin_targets.active_sessions_link_aria', {
            name: props.target.name,
          })}
        >
          {activeSessionsLabel(props.target.active_sessions, props.t)}
        </button>
      </div>
    </div>
  );
}
