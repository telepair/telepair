// web/src/components/Toast.tsx
import { For, Show } from 'solid-js';
import { toast } from '../stores/toast';
import type { Toast as ToastData } from '../stores/toast';
import { useI18n } from '../i18n';

export default function ToastContainer() {
  const { t } = useI18n();
  return (
    <div class="toast-container" role="region" aria-label={t('toast.region_label')}>
      <ol aria-live="polite" aria-atomic="false">
        <For each={toast.list()}>
          {(item) => <ToastItem toast={item} />}
        </For>
      </ol>
      <style>{`
        .toast-container {
          position: fixed;
          top: 16px;
          right: 16px;
          z-index: 9999;
          pointer-events: none;
        }
        .toast-container ol {
          list-style: none;
          display: flex;
          flex-direction: column;
          gap: 8px;
          padding: 0;
          margin: 0;
        }
        .toast {
          pointer-events: auto;
          display: flex;
          align-items: center;
          gap: 10px;
          min-width: 260px;
          max-width: 380px;
          padding: 10px 12px;
          border: 1px solid var(--border);
          border-left: 3px solid var(--text-secondary);
          border-radius: 8px;
          background: var(--bg-secondary);
          color: var(--text-primary);
          font-size: 13px;
          box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
          animation: toast-in 0.18s ease-out;
        }
        .toast[data-variant="info"]    { border-left-color: var(--accent); }
        .toast[data-variant="success"] { border-left-color: var(--success); }
        .toast[data-variant="warning"] { border-left-color: var(--warning); }
        .toast[data-variant="error"]   { border-left-color: var(--error); }

        .toast-text {
          flex: 1;
          line-height: 1.4;
          word-break: break-word;
        }
        .toast-action {
          padding: 4px 10px;
          font-size: 12px;
          background: var(--bg-tertiary);
          border: 1px solid var(--border);
          border-radius: 4px;
          color: var(--text-primary);
          cursor: pointer;
        }
        .toast-action:hover { background: var(--border); }
        .toast-close {
          padding: 0;
          width: 20px;
          height: 20px;
          line-height: 18px;
          text-align: center;
          font-size: 16px;
          background: transparent;
          border: none;
          color: var(--text-secondary);
          border-radius: 4px;
          cursor: pointer;
        }
        .toast-close:hover {
          color: var(--text-primary);
          background: var(--bg-tertiary);
        }
        @keyframes toast-in {
          from { opacity: 0; transform: translateY(-6px); }
          to   { opacity: 1; transform: translateY(0); }
        }
      `}</style>
    </div>
  );
}

function ToastItem(props: { toast: ToastData }) {
  const { t } = useI18n();
  const handleAction = () => {
    props.toast.action?.onClick();
    toast.dismiss(props.toast.id);
  };
  return (
    <li class="toast" data-variant={props.toast.variant} role="status">
      <span class="toast-text">{props.toast.text}</span>
      <Show when={props.toast.action}>
        {(action) => (
          <button class="toast-action" type="button" onClick={handleAction}>
            {action().label}
          </button>
        )}
      </Show>
      <Show when={props.toast.dismissible}>
        <button
          class="toast-close"
          type="button"
          aria-label={t('common.dismiss')}
          onClick={() => toast.dismiss(props.toast.id)}
        >
          ×
        </button>
      </Show>
    </li>
  );
}
