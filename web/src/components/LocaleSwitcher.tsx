// web/src/components/LocaleSwitcher.tsx
//
// Two-locale toggle: 中文 | English. Renders inline so it fits both
// the topbar (Dashboard / Session) and the card footer (Login / Join).
//
// Why a single component with a `variant` prop instead of two separate
// components: the only thing that changes between placements is
// padding/font-size — every interactive bit (state read, click handler,
// active styling, ARIA roles) is shared. Splitting would duplicate the
// behaviour without buying any clarity.
//
// Why two `<button role="radio">`s instead of a `<select>`: with only
// two options, a dropdown is one extra click for zero space saved, and
// the active state isn't visible without opening it. Twin buttons are
// also easier to keyboard-navigate (Tab + Enter) and screen-readers
// announce the current selection via `aria-checked` without any
// custom hint.

import { useI18n, type Locale } from '../i18n';

interface LocaleSwitcherProps {
  /** `topbar` is the compact pill used inside Dashboard / Session
   *  headers. `card` is the larger footer block used on the Login /
   *  Join cards where vertical space is cheap. */
  variant: 'topbar' | 'card';
}

const LOCALES: Array<{ code: Locale; labelKey: 'locale.switch_zh' | 'locale.switch_en' }> = [
  { code: 'zh', labelKey: 'locale.switch_zh' },
  { code: 'en', labelKey: 'locale.switch_en' },
];

export default function LocaleSwitcher(props: LocaleSwitcherProps) {
  const { locale, setLocale, t } = useI18n();

  return (
    <div
      class={`locale-switcher locale-switcher-${props.variant}`}
      role="radiogroup"
      aria-label={t('locale.switch_aria')}
    >
      {LOCALES.map((opt) => (
        <button
          type="button"
          role="radio"
          aria-checked={locale() === opt.code}
          class={locale() === opt.code ? 'locale-btn active' : 'locale-btn'}
          onClick={() => setLocale(opt.code)}
        >
          {t(opt.labelKey)}
        </button>
      ))}
      <style>{`
        .locale-switcher {
          display: inline-flex;
          gap: 2px;
          align-items: center;
        }
        .locale-switcher-topbar {
          padding: 2px;
          background: var(--bg-tertiary);
          border-radius: 999px;
        }
        .locale-switcher-card {
          margin-top: 16px;
          justify-content: center;
          padding: 4px;
          background: var(--bg-tertiary);
          border-radius: 999px;
        }
        .locale-btn {
          background: transparent;
          border: none;
          padding: 4px 12px;
          font-size: 12px;
          color: var(--text-secondary);
          cursor: pointer;
          border-radius: 999px;
          transition: background 0.15s, color 0.15s;
        }
        .locale-switcher-card .locale-btn {
          padding: 6px 16px;
          font-size: 13px;
        }
        .locale-btn:hover {
          color: var(--text-primary);
        }
        .locale-btn.active {
          background: var(--bg-primary);
          color: var(--accent);
          font-weight: 600;
        }
      `}</style>
    </div>
  );
}
