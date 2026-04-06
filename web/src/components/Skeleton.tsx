// web/src/components/Skeleton.tsx
import type { JSX } from 'solid-js';

export type SkeletonProps = {
  width?: string;
  height?: string;
  radius?: string;
  class?: string;
  style?: JSX.CSSProperties;
};

/**
 * Thin wrapper around the shared .skeleton CSS rule in index.css. Renders a
 * placeholder block with a shimmering background animation. Size via props or
 * via a wrapping layout — the element defaults to 100% width × 1em height.
 */
export default function Skeleton(props: SkeletonProps) {
  return (
    <div
      class={`skeleton ${props.class ?? ''}`}
      aria-hidden="true"
      style={{
        width: props.width ?? '100%',
        height: props.height ?? '1em',
        'border-radius': props.radius ?? '4px',
        ...props.style,
      }}
    />
  );
}

/**
 * Dashboard target-card placeholder: a small set of skeleton blocks arranged
 * to mimic the real card layout, so the grid doesn't reflow when data arrives.
 */
export function TargetCardSkeleton() {
  return (
    <div class="skeleton-card">
      <Skeleton width="60%" height="14px" />
      <Skeleton width="40%" height="11px" style={{ 'margin-top': '6px' }} />
      <div style={{ display: 'flex', gap: '4px', 'margin-top': '10px' }}>
        <Skeleton width="40px" height="14px" radius="12px" />
        <Skeleton width="52px" height="14px" radius="12px" />
      </div>
      <style>{`
        .skeleton-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 8px;
          padding: 16px;
        }
      `}</style>
    </div>
  );
}
