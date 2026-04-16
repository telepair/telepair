// web/src/components/SettingsPanel.tsx
import { createSignal, onCleanup, Show, For } from 'solid-js';
import { useI18n } from '../i18n';
import {
  terminalSettings,
  setTheme,
  setFontSize,
  setFontFamily,
  setCursorStyle,
  setCursorBlink,
  resetSettings,
} from '../stores/settings';
import type { CursorStyle } from '../stores/settings';
import { THEME_IDS, FONT_FAMILIES, themes } from '../lib/terminal-themes';
import type { ThemeId, FontFamilyId } from '../lib/terminal-themes';

const THEME_LABEL_KEYS: Record<ThemeId, string> = {
  'github-dark': 'settings.theme_github_dark',
  'github-light': 'settings.theme_github_light',
  dracula: 'settings.theme_dracula',
  monokai: 'settings.theme_monokai',
  'solarized-dark': 'settings.theme_solarized_dark',
};

const FONT_LABEL_KEYS: Record<FontFamilyId, string> = {
  'jetbrains-mono': 'settings.font_jetbrains_mono',
  'fira-code': 'settings.font_fira_code',
  'source-code-pro': 'settings.font_source_code_pro',
  'cascadia-code': 'settings.font_cascadia_code',
  'system-mono': 'settings.font_system_mono',
};

const CURSOR_STYLES: CursorStyle[] = ['block', 'underline', 'bar'];
const CURSOR_LABEL_KEYS: Record<CursorStyle, string> = {
  block: 'settings.cursor_block',
  underline: 'settings.cursor_underline',
  bar: 'settings.cursor_bar',
};

export default function SettingsPanel() {
  const { t } = useI18n();
  const [open, setOpen] = createSignal(false);
  let panelRef: HTMLDivElement | undefined;

  const handleClickOutside = (e: MouseEvent) => {
    if (panelRef && !panelRef.contains(e.target as Node)) {
      setOpen(false);
    }
  };

  const toggle = () => {
    const next = !open();
    setOpen(next);
    if (next) {
      document.addEventListener('mousedown', handleClickOutside);
    } else {
      document.removeEventListener('mousedown', handleClickOutside);
    }
  };

  onCleanup(() => document.removeEventListener('mousedown', handleClickOutside));

  return (
    <div class="settings-anchor" ref={panelRef}>
      <button
        type="button"
        class="settings-gear"
        aria-label={t('settings.aria_open' as any)}
        aria-expanded={open()}
        onClick={toggle}
      >
        ⚙
      </button>
      <Show when={open()}>
        <div class="settings-panel" role="dialog" aria-label={t('settings.title' as any)}>
          <div class="settings-heading">{t('settings.title' as any)}</div>

          {/* Theme swatches */}
          <div class="settings-section">
            <label class="settings-label">{t('settings.theme' as any)}</label>
            <div class="theme-swatches">
              <For each={[...THEME_IDS]}>
                {(id) => (
                  <button
                    type="button"
                    class="theme-swatch"
                    classList={{ active: terminalSettings().theme === id }}
                    title={t(THEME_LABEL_KEYS[id] as any)}
                    onClick={() => setTheme(id)}
                    style={{
                      background: themes[id].background,
                      color: themes[id].foreground,
                      'border-color': terminalSettings().theme === id ? 'var(--accent)' : 'var(--border)',
                    }}
                  >
                    Aa
                  </button>
                )}
              </For>
            </div>
          </div>

          {/* Font size */}
          <div class="settings-section">
            <label class="settings-label">{t('settings.font_size' as any)}</label>
            <div class="font-size-stepper">
              <button
                type="button"
                class="stepper-btn"
                onClick={() => setFontSize(terminalSettings().fontSize - 1)}
                disabled={terminalSettings().fontSize <= 10}
              >
                −
              </button>
              <span class="stepper-value">{terminalSettings().fontSize}</span>
              <button
                type="button"
                class="stepper-btn"
                onClick={() => setFontSize(terminalSettings().fontSize + 1)}
                disabled={terminalSettings().fontSize >= 24}
              >
                +
              </button>
            </div>
          </div>

          {/* Font family */}
          <div class="settings-section">
            <label class="settings-label">{t('settings.font_family' as any)}</label>
            <select
              class="settings-select"
              value={terminalSettings().fontFamily}
              onChange={(e) => setFontFamily(e.currentTarget.value as FontFamilyId)}
            >
              <For each={[...FONT_FAMILIES]}>
                {(f) => (
                  <option value={f.id}>{t(FONT_LABEL_KEYS[f.id] as any)}</option>
                )}
              </For>
            </select>
          </div>

          {/* Cursor style */}
          <div class="settings-section">
            <label class="settings-label">{t('settings.cursor_style' as any)}</label>
            <div class="cursor-style-group">
              <For each={CURSOR_STYLES}>
                {(style) => (
                  <button
                    type="button"
                    class="cursor-style-btn"
                    classList={{ active: terminalSettings().cursorStyle === style }}
                    onClick={() => setCursorStyle(style)}
                  >
                    {t(CURSOR_LABEL_KEYS[style] as any)}
                  </button>
                )}
              </For>
            </div>
          </div>

          {/* Cursor blink */}
          <div class="settings-section settings-row">
            <label class="settings-label">{t('settings.cursor_blink' as any)}</label>
            <button
              type="button"
              class="toggle-switch"
              role="switch"
              aria-checked={terminalSettings().cursorBlink}
              onClick={() => setCursorBlink(!terminalSettings().cursorBlink)}
            >
              <span class="toggle-knob" />
            </button>
          </div>

          {/* Reset */}
          <button
            type="button"
            class="settings-reset"
            onClick={resetSettings}
          >
            {t('settings.reset' as any)}
          </button>
        </div>
      </Show>
    </div>
  );
}
