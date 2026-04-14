import { Show, For } from 'solid-js';
import type { ValidateTargetsResult } from '../lib/protocol';
import { useI18n } from '../i18n';

interface ReloadConfirmDialogProps {
  result: ValidateTargetsResult | null;
  reloading: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export default function ReloadConfirmDialog(props: ReloadConfirmDialogProps) {
  const { t } = useI18n();

  const diff = () => props.result?.diff;
  const blocked = () => props.result?.blocked ?? [];
  const isBlocked = () => blocked().length > 0;

  return (
    <Show when={props.result !== null}>
      <div class="dialog-overlay" onClick={props.onCancel}>
        <div
          class="dialog"
          role="dialog"
          aria-modal="true"
          aria-label={t('admin_targets.validate_title')}
          onClick={(e) => e.stopPropagation()}
          data-testid="reload-confirm-dialog"
        >
          <h2 class="dialog-title">{t('admin_targets.validate_title')}</h2>

          <div class="diff-section">
            <Show when={(diff()?.added.length ?? 0) > 0}>
              <div class="diff-group diff-added">
                <span class="diff-label">Added</span>
                <ul class="diff-list">
                  <For each={diff()?.added ?? []}>
                    {(name) => <li class="diff-row">{name}</li>}
                  </For>
                </ul>
              </div>
            </Show>

            <Show when={(diff()?.changed.length ?? 0) > 0}>
              <div class="diff-group diff-changed">
                <span class="diff-label">Changed</span>
                <ul class="diff-list">
                  <For each={diff()?.changed ?? []}>
                    {(name) => <li class="diff-row">{name}</li>}
                  </For>
                </ul>
              </div>
            </Show>

            <Show when={(diff()?.removed.length ?? 0) > 0}>
              <div class="diff-group diff-removed">
                <span class="diff-label">Removed</span>
                <ul class="diff-list">
                  <For each={diff()?.removed ?? []}>
                    {(name) => {
                      const blockedTarget = blocked().find((b) => b.target === name);
                      return (
                        <li class="diff-row" data-blocked={blockedTarget ? 'true' : 'false'}>
                          {name}
                          <Show when={blockedTarget}>
                            <span class="blocked-badge">
                              {t('admin_targets.validate_blocked_sessions', {
                                count: String(blockedTarget!.active_sessions),
                              })}
                            </span>
                          </Show>
                        </li>
                      );
                    }}
                  </For>
                </ul>
              </div>
            </Show>

            <Show when={(diff()?.unchanged.length ?? 0) > 0}>
              <div class="diff-group diff-unchanged">
                <span class="diff-label">Unchanged</span>
                <ul class="diff-list">
                  <For each={diff()?.unchanged ?? []}>
                    {(name) => <li class="diff-row">{name}</li>}
                  </For>
                </ul>
              </div>
            </Show>
          </div>

          <Show when={isBlocked()}>
            <p class="blocked-hint">{t('admin_targets.validate_blocked_hint')}</p>
          </Show>

          <div class="dialog-actions">
            <button
              type="button"
              onClick={props.onCancel}
              disabled={props.reloading}
            >
              {t('common.cancel')}
            </button>
            <button
              type="button"
              class="primary"
              onClick={props.onConfirm}
              disabled={props.reloading || isBlocked()}
              data-testid="reload-confirm-apply"
            >
              {props.reloading ? t('admin_targets.reloading') : t('admin_targets.validate_apply')}
            </button>
          </div>
        </div>
      </div>

      <style>{`
        .dialog-overlay {
          position: fixed;
          inset: 0;
          background: rgba(0, 0, 0, 0.6);
          display: flex;
          align-items: center;
          justify-content: center;
          z-index: 1000;
        }
        .dialog {
          background: var(--bg-primary);
          border: 1px solid var(--border);
          border-radius: 10px;
          padding: 24px;
          width: 520px;
          max-width: calc(100vw - 32px);
          max-height: calc(100vh - 64px);
          overflow-y: auto;
          display: flex;
          flex-direction: column;
          gap: 16px;
        }
        .dialog-title {
          font-size: 16px;
          font-weight: 700;
          margin: 0;
        }
        .diff-section {
          display: flex;
          flex-direction: column;
          gap: 12px;
        }
        .diff-group {
          display: flex;
          flex-direction: column;
          gap: 4px;
        }
        .diff-label {
          font-size: 11px;
          text-transform: uppercase;
          letter-spacing: 0.04em;
          font-weight: 700;
        }
        .diff-added .diff-label { color: var(--success); }
        .diff-changed .diff-label { color: var(--accent); }
        .diff-removed .diff-label { color: var(--error, #e05c5c); }
        .diff-unchanged .diff-label { color: var(--text-secondary); }
        .diff-list {
          margin: 0;
          padding-left: 16px;
          list-style: disc;
          display: flex;
          flex-direction: column;
          gap: 2px;
        }
        .diff-row {
          font-family: var(--font-mono);
          font-size: 13px;
          color: var(--text-primary);
          display: flex;
          align-items: center;
          gap: 8px;
        }
        .diff-row[data-blocked='true'] {
          color: var(--error, #e05c5c);
        }
        .blocked-badge {
          font-family: inherit;
          font-size: 11px;
          padding: 1px 7px;
          border-radius: 999px;
          background: rgba(224, 92, 92, 0.15);
          color: var(--error, #e05c5c);
        }
        .blocked-hint {
          font-size: 13px;
          color: var(--warning);
          margin: 0;
          padding: 10px 12px;
          background: rgba(210, 153, 34, 0.1);
          border-radius: 6px;
          border: 1px solid rgba(210, 153, 34, 0.3);
        }
        .dialog-actions {
          display: flex;
          justify-content: flex-end;
          gap: 8px;
        }
        .dialog-actions button {
          font-size: 13px;
          padding: 7px 16px;
          border-radius: 6px;
          cursor: pointer;
        }
        .dialog-actions button:disabled {
          opacity: 0.6;
          cursor: default;
        }
      `}</style>
    </Show>
  );
}
