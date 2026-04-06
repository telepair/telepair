// web/src/components/Banner.tsx
import { Show } from 'solid-js';
import type { JSX } from 'solid-js';

export type BannerVariant = 'info' | 'success' | 'warning' | 'error';

export default function Banner(props: {
  variant?: BannerVariant;
  onDismiss?: () => void;
  children: JSX.Element;
}) {
  const variant = () => props.variant ?? 'error';
  return (
    <div class="banner" data-variant={variant()} role="alert">
      <span class="banner-text">{props.children}</span>
      <Show when={props.onDismiss}>
        <button
          class="banner-close"
          type="button"
          aria-label="Dismiss notification"
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
          background: rgba(88, 166, 255, 0.12);
          color: var(--accent);
          border-bottom-color: rgba(88, 166, 255, 0.3);
        }
        .banner[data-variant="success"] {
          background: rgba(63, 185, 80, 0.12);
          color: var(--success);
          border-bottom-color: rgba(63, 185, 80, 0.3);
        }
        .banner[data-variant="warning"] {
          background: rgba(210, 153, 34, 0.15);
          color: var(--warning);
          border-bottom-color: rgba(210, 153, 34, 0.3);
        }
        .banner[data-variant="error"] {
          background: rgba(248, 81, 73, 0.15);
          color: var(--error);
          border-bottom-color: rgba(248, 81, 73, 0.3);
        }
        .banner-text {
          flex: 1;
          line-height: 1.4;
          word-break: break-word;
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
