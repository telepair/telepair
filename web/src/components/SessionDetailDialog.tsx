// web/src/components/SessionDetailDialog.tsx
//
// Modal dialog showing the audit timeline for a single session. Backs
// the "click a closed session row" affordance on the dashboard, which
// before v0.1.1 was inert (closed sessions were dead history with no
// detail surface). The dialog is owner-only on the server side; the
// dashboard never offers this affordance to users who don't own the
// session, so the 403 path is a defense-in-depth fallback rather than
// an expected UX state.
//
// The audit timeline is rendered newest-first. Each row carries:
//   - a one-line label (event type, translated)
//   - the actor name (denormalized snapshot from the audit row, so
//     renaming a user later doesn't rewrite history)
//   - the timestamp in the user's locale
//   - a foldable JSON detail blob for events that carry one
//
// We deliberately do NOT try to pretty-print every event type with a
// bespoke template. The detail JSON shape varies per event type and
// isn't a stable contract — surfacing it raw keeps the UI honest and
// keeps this component immune to backend taxonomy changes.

import {
  createSignal,
  createResource,
  createMemo,
  createEffect,
  For,
  Show,
} from 'solid-js';
import { api, errorMessage } from '../lib/api';
import { type AuditEvent, type Session } from '../lib/protocol';
import { eventLabel, formatTs } from '../lib/audit';
import { useI18n, type Translator } from '../i18n';

interface SessionDetailDialogProps {
  /** Session whose timeline to render. `null` keeps the dialog closed. */
  session: Session | null;
  onClose: () => void;
}

/** Best-effort one-line summary of an audit row's `detail` blob. We
 *  pluck the few keys we know about (`role`, `reason`, `as_guest`) so
 *  the timeline conveys the headline fact without the user clicking
 *  through to the raw JSON. Anything we don't recognise is omitted —
 *  the raw JSON drawer below covers the rest. */
function detailSummary(t: Translator, row: AuditEvent): string {
  const d = row.detail;
  if (d == null || typeof d !== 'object') return '';
  const obj = d as Record<string, unknown>;
  const parts: string[] = [];
  if (typeof obj.role === 'string') {
    parts.push(t('session_detail.detail_role', { role: obj.role }));
  }
  if (typeof obj.reason === 'string') {
    parts.push(t('session_detail.detail_reason', { reason: obj.reason }));
  }
  if (typeof obj.as_guest === 'boolean' && obj.as_guest) {
    parts.push(t('session_detail.detail_as_guest'));
  }
  return parts.join(' · ');
}

export default function SessionDetailDialog(props: SessionDetailDialogProps) {
  const { t } = useI18n();
  // Per-row "show raw JSON" toggle. Stored as a Set of row ids so the
  // toggle survives re-renders without resetting the entire timeline
  // when one row's drawer flips.
  const [expanded, setExpanded] = createSignal<Set<number>>(new Set());

  // `createResource` keyed on the session id: any time the parent
  // passes a new session, the resource refetches automatically. When
  // the parent closes the dialog (`session=null`), the source returns
  // `false` and the resource resolves to `undefined`, which the JSX
  // collapses to "no rows" with no extra plumbing.
  const [audit, { mutate: mutateAudit }] = createResource(
    () => props.session?.id ?? null,
    async (sessionId): Promise<AuditEvent[]> => {
      if (!sessionId) return [];
      // Reset the per-row expansion state on every fresh fetch — a
      // newly opened dialog should never inherit drawers that were
      // open the last time we looked at a different session.
      setExpanded(new Set<number>());
      return api.listSessionAudit(sessionId);
    },
  );

  // Clear the cached audit rows on every session-id change. Without
  // this, Solid's `createResource` keeps the last-resolved value visible
  // while the new fetch is in flight — fine for a refresh, wrong for a
  // source swap. The visible bug: open session A, close, open session
  // B, and for a frame or two the dialog header shows session B while
  // the timeline still lists A's events. `mutate(undefined)` drops the
  // cached value so the `audit.loading` branch wins until B resolves.
  createEffect(() => {
    // Track the source signal — referencing it inside the effect is
    // what subscribes us to changes.
    void props.session?.id;
    mutateAudit(undefined);
  });

  // Read the timeline through a guarded memo so JSX never calls the
  // `audit()` accessor while the resource is in `errored` state.
  // SolidJS re-throws the underlying error from every resource accessor
  // read while errored, which would crash the dialog before the
  // `audit.error` branch above gets a chance to render the friendly
  // message. Returning an empty array during loading/error lets the
  // `<Show when={auditRows().length > 0}>` branch collapse to the
  // fallback without touching the throwing path.
  const auditRows = createMemo<AuditEvent[]>(() => {
    if (audit.loading || audit.error) return [];
    const rows = audit();
    return rows ?? [];
  });

  const toggleExpanded = (id: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  return (
    <Show when={props.session}>
      {(session) => (
        <div
          class="dialog-backdrop"
          onClick={props.onClose}
          data-testid="session-detail-backdrop"
        >
          <div
            class="dialog session-detail-dialog"
            onClick={(e) => e.stopPropagation()}
          >
            <header class="detail-header">
              <h3>{t('session_detail.title')}</h3>
              <code class="detail-id">{session().id}</code>
            </header>

            <dl class="detail-meta">
              <div>
                <dt>{t('session_detail.target')}</dt>
                <dd>{session().target_name}</dd>
              </div>
              <div>
                <dt>{t('session_detail.created_at')}</dt>
                <dd>{formatTs(session().created_at)}</dd>
              </div>
              <Show when={session().closed_at}>
                <div>
                  <dt>{t('session_detail.closed_at')}</dt>
                  <dd>{formatTs(session().closed_at!)}</dd>
                </div>
              </Show>
            </dl>

            <h4 class="timeline-heading">
              {t('session_detail.timeline_heading')}
            </h4>

            <Show when={audit.loading}>
              <p class="hint">{t('session_detail.loading')}</p>
            </Show>

            <Show when={audit.error}>
              <p class="manage-error" data-testid="session-detail-error">
                {t('session_detail.load_failed', {
                  msg: errorMessage(audit.error),
                })}
              </p>
            </Show>

            <Show
              when={auditRows().length > 0}
              fallback={
                <Show when={!audit.loading && !audit.error}>
                  <p class="hint" data-testid="session-detail-empty">
                    {t('session_detail.timeline_empty')}
                  </p>
                </Show>
              }
            >
              <ol class="timeline" data-testid="session-detail-timeline">
                <For each={auditRows()}>
                  {(row) => {
                    const summary = detailSummary(t, row);
                    const id = row.id ?? -1;
                    return (
                      <li class="timeline-row" data-event-type={row.event_type}>
                        <div class="timeline-row-main">
                          <span class="timeline-ts">{formatTs(row.ts)}</span>
                          <span class="timeline-event">
                            {eventLabel(t, row.event_type)}
                          </span>
                          <Show when={row.actor_name}>
                            <span class="timeline-actor">
                              {row.actor_name}
                            </span>
                          </Show>
                        </div>
                        <Show when={summary}>
                          <div class="timeline-summary">{summary}</div>
                        </Show>
                        <Show when={row.detail != null && id >= 0}>
                          <button
                            type="button"
                            class="timeline-detail-toggle"
                            onClick={() => toggleExpanded(id)}
                          >
                            {expanded().has(id)
                              ? t('session_detail.detail_hide')
                              : t('session_detail.detail_show')}
                          </button>
                          <Show when={expanded().has(id)}>
                            <pre class="timeline-detail-json">
                              {JSON.stringify(row.detail, null, 2)}
                            </pre>
                          </Show>
                        </Show>
                      </li>
                    );
                  }}
                </For>
              </ol>
            </Show>

            <button
              type="button"
              class="primary detail-close"
              onClick={props.onClose}
            >
              {t('common.done')}
            </button>
          </div>

          <style>{`
            .dialog-backdrop {
              position: fixed; inset: 0;
              background: rgba(0,0,0,0.5);
              display: flex; align-items: center; justify-content: center;
              z-index: 100;
            }
            .session-detail-dialog {
              background: var(--bg-secondary);
              border: 1px solid var(--border);
              border-radius: 12px;
              padding: 24px;
              width: 560px; max-width: 92vw;
              max-height: 86vh;
              display: flex; flex-direction: column;
              gap: 12px;
            }
            .detail-header {
              display: flex; align-items: baseline; gap: 12px;
            }
            .detail-header h3 {
              font-size: 16px; font-weight: 600; margin: 0;
            }
            .detail-id {
              font-family: var(--font-mono);
              font-size: 12px;
              color: var(--accent);
            }
            .detail-meta {
              display: grid;
              grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
              gap: 8px 16px;
              padding: 12px;
              background: var(--bg-tertiary);
              border-radius: 8px;
              margin: 0;
            }
            .detail-meta dt {
              font-size: 11px;
              text-transform: uppercase;
              color: var(--text-secondary);
              margin-bottom: 2px;
            }
            .detail-meta dd {
              margin: 0;
              font-size: 13px;
              color: var(--text-primary);
            }
            .timeline-heading {
              font-size: 12px;
              font-weight: 600;
              color: var(--text-secondary);
              margin: 4px 0;
            }
            .timeline {
              list-style: none;
              padding: 0;
              margin: 0;
              overflow-y: auto;
              flex: 1;
              min-height: 80px;
              max-height: 50vh;
              border: 1px solid var(--border);
              border-radius: 8px;
              background: var(--bg-primary);
            }
            .timeline-row {
              padding: 10px 12px;
              border-bottom: 1px solid var(--border);
              font-size: 13px;
              display: flex;
              flex-direction: column;
              gap: 4px;
            }
            .timeline-row:last-child { border-bottom: none; }
            .timeline-row-main {
              display: flex;
              gap: 12px;
              align-items: baseline;
              flex-wrap: wrap;
            }
            .timeline-ts {
              font-family: var(--font-mono);
              font-size: 11px;
              color: var(--text-secondary);
              white-space: nowrap;
            }
            .timeline-event {
              font-weight: 600;
              color: var(--text-primary);
            }
            .timeline-actor {
              color: var(--text-secondary);
              font-size: 12px;
            }
            .timeline-summary {
              font-size: 12px;
              color: var(--text-secondary);
              padding-left: 0;
            }
            .timeline-detail-toggle {
              align-self: flex-start;
              font-size: 11px;
              padding: 2px 8px;
              background: transparent;
              border: 1px solid var(--border);
              color: var(--text-secondary);
              border-radius: 999px;
              cursor: pointer;
            }
            .timeline-detail-toggle:hover { color: var(--text-primary); }
            .timeline-detail-json {
              font-family: var(--font-mono);
              font-size: 11px;
              padding: 8px 10px;
              background: var(--bg-tertiary);
              border-radius: 6px;
              overflow-x: auto;
              margin: 0;
              white-space: pre-wrap;
              word-break: break-word;
            }
            .hint {
              font-size: 12px;
              color: var(--text-secondary);
            }
            .manage-error {
              font-size: 12px;
              color: var(--error);
              padding: 8px 12px;
              background: rgba(248, 81, 73, 0.1);
              border-radius: 6px;
            }
            .detail-close {
              align-self: flex-end;
              margin-top: 4px;
            }
          `}</style>
        </div>
      )}
    </Show>
  );
}
