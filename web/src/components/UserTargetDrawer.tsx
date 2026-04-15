// web/src/components/UserTargetDrawer.tsx
import { createEffect, createSignal, Show } from 'solid-js';
import type { UserTargetInfo } from '../lib/protocol';
import { api, errorMessage } from '../lib/api';
import { useI18n } from '../i18n';

interface UserTargetDrawerProps {
  /** `null` = closed, `undefined` = create mode, object = edit mode */
  target: UserTargetInfo | null | undefined;
  onClose: () => void;
  onSaved: (target: UserTargetInfo) => void;
  onDeleted: (id: string) => void;
}

/** Parse space-separated args string, respecting basic quoting. */
function parseArgs(raw: string): string[] {
  return raw.trim() ? raw.trim().split(/\s+/) : [];
}

/** Parse KEY=value lines into an object. Ignores blank lines and comments. */
function parseEnv(raw: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq < 1) continue;
    result[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1);
  }
  return result;
}

function envToString(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([k, v]) => `${k}=${v}`)
    .join('\n');
}

export default function UserTargetDrawer(props: UserTargetDrawerProps) {
  const { t } = useI18n();
  const isOpen = () => props.target !== null;
  const isEdit = () => props.target != null && props.target !== undefined && typeof props.target === 'object';

  const [name, setName] = createSignal('');
  const [display, setDisplay] = createSignal('');
  const [command, setCommand] = createSignal('');
  const [argsStr, setArgsStr] = createSignal('');
  const [envStr, setEnvStr] = createSignal('');
  const [tagsStr, setTagsStr] = createSignal('');
  const [saving, setSaving] = createSignal(false);
  const [deleting, setDeleting] = createSignal(false);
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const [saveError, setSaveError] = createSignal('');
  const [deleteError, setDeleteError] = createSignal('');

  // Reset form whenever the drawer opens with a new target.
  createEffect(() => {
    const tgt = props.target;
    if (tgt === null) return; // closed, don't reset
    if (tgt === undefined) {
      // create mode
      setName('');
      setDisplay('');
      setCommand('');
      setArgsStr('');
      setEnvStr('');
      setTagsStr('');
    } else {
      // edit mode
      setName(tgt.name);
      setDisplay(tgt.display);
      setCommand(tgt.command);
      setArgsStr(tgt.args.join(' '));
      setEnvStr(envToString(tgt.env));
      setTagsStr(tgt.tags.join(' '));
    }
    setSaveError('');
    setDeleteError('');
    setConfirmDelete(false);
  });

  const handleSave = async (e: Event) => {
    e.preventDefault();
    setSaving(true);
    setSaveError('');
    try {
      const params = {
        display: display(),
        command: command(),
        args: parseArgs(argsStr()),
        env: parseEnv(envStr()),
        tags: parseArgs(tagsStr()),
      };
      let saved: UserTargetInfo;
      if (isEdit()) {
        saved = await api.updateUserTarget((props.target as UserTargetInfo).id, params);
      } else {
        saved = await api.createUserTarget({ name: name(), ...params });
      }
      props.onSaved(saved);
    } catch (err) {
      setSaveError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!isEdit()) return;
    setDeleting(true);
    setDeleteError('');
    try {
      const t = props.target as UserTargetInfo;
      await api.deleteUserTarget(t.id);
      props.onDeleted(t.id);
    } catch (err) {
      setDeleteError(errorMessage(err));
      setConfirmDelete(false);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Show when={isOpen()}>
      <div class="drawer-backdrop" onClick={props.onClose}>
        <div
          class="drawer"
          role="dialog"
          aria-modal="true"
          aria-labelledby="ut-drawer-title"
          onClick={(e) => e.stopPropagation()}
        >
          <div class="drawer-header">
            <h3 id="ut-drawer-title">
              {isEdit() ? t('user_target.edit_title') : t('user_target.create_title')}
            </h3>
            <button class="close-btn" onClick={props.onClose} aria-label="Close">×</button>
          </div>

          <form class="drawer-body" onSubmit={handleSave}>
            <Show when={!isEdit()}>
              <label for="ut-name">{t('user_target.name_label')}</label>
              <input
                id="ut-name"
                type="text"
                placeholder={t('user_target.name_placeholder')}
                value={name()}
                onInput={(e) => setName(e.currentTarget.value)}
                pattern="[a-z0-9][a-z0-9\-]*"
                required
              />
              <p class="field-hint">{t('user_target.name_hint')}</p>
            </Show>

            <label for="ut-display">{t('user_target.display_label')}</label>
            <input
              id="ut-display"
              type="text"
              placeholder={t('user_target.display_placeholder')}
              value={display()}
              onInput={(e) => setDisplay(e.currentTarget.value)}
              required
            />

            <label for="ut-command">{t('user_target.command_label')}</label>
            <input
              id="ut-command"
              type="text"
              placeholder={t('user_target.command_placeholder')}
              value={command()}
              onInput={(e) => setCommand(e.currentTarget.value)}
              required
            />

            <label for="ut-args">{t('user_target.args_label')}</label>
            <input
              id="ut-args"
              type="text"
              placeholder={t('user_target.args_placeholder')}
              value={argsStr()}
              onInput={(e) => setArgsStr(e.currentTarget.value)}
            />
            <p class="field-hint">{t('user_target.args_hint')}</p>

            <label for="ut-env">{t('user_target.env_label')}</label>
            <textarea
              id="ut-env"
              rows={4}
              placeholder={t('user_target.env_placeholder')}
              value={envStr()}
              onInput={(e) => setEnvStr(e.currentTarget.value)}
            />
            <p class="field-hint">{t('user_target.env_hint')}</p>

            <label for="ut-tags">{t('user_target.tags_label')}</label>
            <input
              id="ut-tags"
              type="text"
              placeholder={t('user_target.tags_placeholder')}
              value={tagsStr()}
              onInput={(e) => setTagsStr(e.currentTarget.value)}
            />
            <p class="field-hint">{t('user_target.tags_hint')}</p>

            <Show when={saveError()}>
              <p class="error-msg">{saveError()}</p>
            </Show>

            <div class="drawer-actions">
              <Show when={isEdit()}>
                <Show
                  when={confirmDelete()}
                  fallback={
                    <button
                      type="button"
                      class="danger-outline"
                      onClick={() => setConfirmDelete(true)}
                    >
                      {t('user_target.delete')}
                    </button>
                  }
                >
                  <div class="confirm-delete">
                    <span class="confirm-delete-label">{t('user_target.delete_confirm')}</span>
                    <button
                      type="button"
                      class="danger"
                      disabled={deleting()}
                      onClick={handleDelete}
                    >
                      {deleting() ? t('user_target.deleting') : t('user_target.delete_yes')}
                    </button>
                    <button
                      type="button"
                      onClick={() => setConfirmDelete(false)}
                      disabled={deleting()}
                    >
                      {t('user_target.delete_no')}
                    </button>
                  </div>
                </Show>
                <Show when={deleteError()}>
                  <p class="error-msg">{deleteError()}</p>
                </Show>
              </Show>

              <div class="save-actions">
                <button type="button" onClick={props.onClose} disabled={saving()}>
                  {t('common.cancel')}
                </button>
                <button
                  type="submit"
                  class="primary"
                  disabled={saving() || !display() || !command() || (!isEdit() && !name())}
                >
                  {saving() ? t('user_target.saving') : t('user_target.save')}
                </button>
              </div>
            </div>
          </form>
        </div>
      </div>

      <style>{`
        .drawer-backdrop {
          position: fixed;
          inset: 0;
          background: rgba(0,0,0,0.45);
          z-index: 100;
          display: flex;
          justify-content: flex-end;
        }
        .drawer {
          background: var(--bg-secondary);
          border-left: 1px solid var(--border);
          width: 420px;
          max-width: 95vw;
          height: 100%;
          overflow-y: auto;
          display: flex;
          flex-direction: column;
        }
        .drawer-header {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 20px 24px 16px;
          border-bottom: 1px solid var(--border);
          position: sticky;
          top: 0;
          background: var(--bg-secondary);
          z-index: 1;
        }
        .drawer-header h3 {
          font-size: 16px;
          font-weight: 600;
        }
        .close-btn {
          background: transparent;
          border: none;
          font-size: 20px;
          color: var(--text-secondary);
          cursor: pointer;
          padding: 2px 6px;
          line-height: 1;
        }
        .close-btn:hover { color: var(--text-primary); background: transparent; }
        .drawer-body {
          padding: 20px 24px;
          display: flex;
          flex-direction: column;
          gap: 4px;
          flex: 1;
        }
        .drawer-body label {
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          margin-top: 10px;
        }
        .drawer-body input, .drawer-body textarea {
          margin-bottom: 2px;
          font-family: var(--font-mono);
          font-size: 13px;
        }
        .field-hint {
          font-size: 11px;
          color: var(--text-secondary);
          margin-bottom: 4px;
        }
        .error-msg {
          color: var(--error);
          font-size: 13px;
          margin-top: 8px;
        }
        .drawer-actions {
          margin-top: 20px;
          display: flex;
          flex-direction: column;
          gap: 10px;
        }
        .save-actions {
          display: flex;
          gap: 8px;
          justify-content: flex-end;
        }
        .danger-outline {
          background: transparent;
          border-color: var(--error);
          color: var(--error);
          font-size: 13px;
          padding: 6px 14px;
          align-self: flex-start;
        }
        .danger-outline:hover {
          background: rgba(255,80,80,0.1);
        }
        .danger {
          background: var(--error);
          border-color: var(--error);
          color: #fff;
          font-size: 13px;
          padding: 6px 14px;
        }
        .confirm-delete {
          display: flex;
          align-items: center;
          gap: 8px;
          flex-wrap: wrap;
        }
        .confirm-delete-label {
          font-size: 13px;
          color: var(--text-secondary);
        }
      `}</style>
    </Show>
  );
}
