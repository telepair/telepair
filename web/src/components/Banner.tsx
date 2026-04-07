// web/src/components/Banner.tsx
import { Show } from 'solid-js';
import type { JSX } from 'solid-js';
import { useI18n } from '../i18n';

export type BannerVariant = 'info' | 'success' | 'warning' | 'error';

export interface BannerAction {
  label: string;
  onClick: () => void;
}

export default function Banner(props: {
  variant?: BannerVariant;
  onDismiss?: () => void;
  action?: BannerAction;
  /** ARIA role — defaults to "alert"; use "status" for non-assertive updates. */
  role?: 'alert' | 'status';
  children: JSX.Element;
}) {
  const { t } = useI18n();
  const variant = () => props.variant ?? 'error';
  return (
    <div class="banner" data-variant={variant()} role={props.role ?? 'alert'}>
      <span class="banner-text">{props.children}</span>
      <Show when={props.action}>
        {(action) => (
          <button class="banner-action" type="button" onClick={action().onClick}>
            {action().label}
          </button>
        )}
      </Show>
      <Show when={props.onDismiss}>
        <button
          class="banner-close"
          type="button"
          aria-label={t('common.dismiss')}
          onClick={props.onDismiss}
        >
          ×
        </button>
      </Show>
      <style>{`
        .banner {
          display: flex;
          align-items: center;
          gap: 12px;
          padding: 8px 16px;
          font-size: 13px;
          border-bottom: 1px solid var(--border);
        }
        .banner[data-variant="info"] {
          --banner-accent: var(--accent);
          background: rgba(88, 166, 255, 0.12);
          color: var(--accent);
          border-bottom-color: rgba(88, 166, 255, 0.3);
        }
        .banner[data-variant="success"] {
          --banner-accent: var(--success);
          background: rgba(63, 185, 80, 0.12);
          color: var(--success);
          border-bottom-color: rgba(63, 185, 80, 0.3);
        }
        .banner[data-variant="warning"] {
          --banner-accent: var(--warning);
          background: rgba(210, 153, 34, 0.15);
          color: var(--warning);
          border-bottom-color: rgba(210, 153, 34, 0.3);
        }
        .banner[data-variant="error"] {
          --banner-accent: var(--error);
          background: rgba(248, 81, 73, 0.15);
          color: var(--error);
          border-bottom-color: rgba(248, 81, 73, 0.3);
        }
        .banner-text {
          flex: 1;
          line-height: 1.4;
          word-break: break-word;
        }
        .banner-action {
          /* Use a variant-scoped custom property instead of currentColor:
             currentColor would resolve against this button's own color
             (which we set to --bg-primary below), not the banner's. */
          padding: 4px 12px;
          font-size: 12px;
          font-weight: 500;
          color: var(--bg-primary);
          background: var(--banner-accent);
          border: 1px solid var(--banner-accent);
          border-radius: 4px;
          cursor: pointer;
        }
        .banner-action:hover {
          filter: brightness(1.15);
        }
        .banner-close {
          padding: 0;
          width: 22px;
          height: 22px;
          line-height: 20px;
          text-align: center;
          font-size: 16px;
          background: transparent;
          border: 1px solid transparent;
          color: inherit;
          border-radius: 4px;
          cursor: pointer;
          opacity: 0.75;
        }
        .banner-close:hover {
          opacity: 1;
          background: rgba(255, 255, 255, 0.08);
        }
      `}</style>
    </div>
  );
}
