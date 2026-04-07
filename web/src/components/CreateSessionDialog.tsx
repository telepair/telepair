import { createEffect, createSignal, Show, For } from 'solid-js';
import type { TargetInfo, InputMode } from '../lib/protocol';
import { useI18n } from '../i18n';

interface CreateSessionDialogProps {
  /** Target the user clicked; `null` means dialog is closed. Keeping this
   * as a prop instead of an open/target pair lets the parent drive the
   * whole modal from a single signal. */
  target: TargetInfo | null;
  /** Last-used mode, used as the default selection whenever the dialog
   * opens. The dialog itself is the authoritative source for the final
   * mode — the parent only passes it through to `onLaunch`. */
  defaultMode: InputMode;
  /** True while the create-session API call is inflight. Used to dim the
   * Launch button so the user doesn't double-submit. */
  busy: boolean;
  onCancel: () => void;
  onLaunch: (mode: InputMode) => void;
}

/**
 * Confirmation + mode-picker modal shown after clicking a target card.
 *
 * Why this exists instead of a one-click launch:
 *   - The default mode matters. Clicking a target straight to launch
 *     would force the server-side default and leave users with no way
 *     to start a "solo" or "collaborative" run without editing storage
 *     manually.
 *   - A modal also prevents accidental launches — target cards are big
 *     click targets and a stray click used to spawn an unwanted PTY.
 *   - Showing the resolved command in the dialog gives the user one
 *     last chance to confirm "yes, that's the shell I meant", which is
 *     useful in multi-target setups (different container shells,
 *     remote hosts, etc.).
 */
export default function CreateSessionDialog(props: CreateSessionDialogProps) {
  const { t } = useI18n();
  const [mode, setMode] = createSignal<InputMode>(props.defaultMode);

  // Reset the local mode every time a new target is opened — otherwise
  // the last selection from a prior dialog session would stick around
  // and silently override the caller's `defaultMode`.
  createEffect(() => {
    if (props.target) {
      setMode(props.defaultMode);
    }
  });

  const handleBackdropClick = () => {
    if (!props.busy) props.onCancel();
  };

  return (
    <Show when={props.target}>
      {(target) => (
        <div class="dialog-backdrop" onClick={handleBackdropClick}>
          <div
            class="dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="create-session-title"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 id="create-session-title">{t('create_session.title')}</h3>
            <div class="target-summary">
              <div class="target-display">{target().display}</div>
              <div class="target-id">{target().name}</div>
              <Show when={target().tags.length > 0}>
                <div class="tags">
                  <For each={target().tags}>
                    {(tag) => <span class="tag">{tag}</span>}
                  </For>
                </div>
              </Show>
            </div>

            <label class="mode-label">{t('create_session.mode_label')}</label>
            <div class="mode-options" role="radiogroup" aria-label={t('create_session.mode_label_aria')}>
              <button
                type="button"
                role="radio"
                aria-checked={mode() === 'multiplexed'}
                class={mode() === 'multiplexed' ? 'mode-btn active' : 'mode-btn'}
                disabled={props.busy}
                onClick={() => setMode('multiplexed')}
              >
                <span class="mode-title">{t('create_session.mode_collaborative')}</span>
                <span class="mode-desc">{t('create_session.mode_collaborative_desc')}</span>
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={mode() === 'serialized'}
                class={mode() === 'serialized' ? 'mode-btn active' : 'mode-btn'}
                disabled={props.busy}
                onClick={() => setMode('serialized')}
              >
                <span class="mode-title">{t('create_session.mode_solo')}</span>
                <span class="mode-desc">{t('create_session.mode_solo_desc')}</span>
              </button>
            </div>

            <div class="dialog-actions">
              <button type="button" onClick={props.onCancel} disabled={props.busy}>
                {t('common.cancel')}
              </button>
              <button
                type="button"
                class="primary"
                disabled={props.busy}
                onClick={() => props.onLaunch(mode())}
              >
                {props.busy ? t('create_session.launching') : t('create_session.launch')}
              </button>
            </div>

            <style>{`
              .dialog-backdrop {
                position: fixed;
                inset: 0;
                background: rgba(0,0,0,0.5);
                display: flex;
                align-items: center;
                justify-content: center;
                z-index: 100;
              }
              .dialog {
                background: var(--bg-secondary);
                border: 1px solid var(--border);
                border-radius: 12px;
                padding: 24px;
                width: 440px;
                max-width: 92vw;
              }
              .dialog h3 {
                font-size: 16px;
                font-weight: 600;
                margin-bottom: 16px;
              }
              .target-summary {
                padding: 12px 14px;
                border: 1px solid var(--border);
                border-radius: 8px;
                background: var(--bg-tertiary);
                margin-bottom: 16px;
              }
              .target-display { font-weight: 600; margin-bottom: 2px; }
              .target-id {
                font-family: var(--font-mono);
                font-size: 12px;
                color: var(--text-secondary);
              }
              .tags { margin-top: 8px; display: flex; gap: 4px; flex-wrap: wrap; }
              .tag {
                font-size: 11px;
                padding: 2px 8px;
                border-radius: 12px;
                background: var(--bg-secondary);
                color: var(--text-secondary);
              }
              .mode-label {
                display: block;
                font-size: 12px;
                font-weight: 600;
                color: var(--text-secondary);
                margin-bottom: 8px;
              }
              .mode-options {
                display: flex;
                gap: 8px;
                margin-bottom: 20px;
              }
              .mode-btn {
                flex: 1;
                padding: 12px;
                text-align: left;
                border-radius: 8px;
                display: flex;
                flex-direction: column;
                gap: 4px;
                background: transparent;
                cursor: pointer;
              }
              .mode-btn:disabled { cursor: default; opacity: 0.6; }
              .mode-btn.active {
                border-color: var(--accent);
                background: rgba(88,166,255,0.1);
              }
              .mode-title { font-weight: 600; font-size: 13px; }
              .mode-desc { font-size: 11px; color: var(--text-secondary); }
              .dialog-actions {
                display: flex;
                gap: 8px;
                justify-content: flex-end;
              }
            `}</style>
          </div>
        </div>
      )}
    </Show>
  );
}
