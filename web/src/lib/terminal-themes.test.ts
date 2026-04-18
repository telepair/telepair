import { describe, expect, it } from 'vitest';
import {
  DEFAULT_FONT_FAMILY,
  DEFAULT_THEME,
  FONT_FAMILIES,
  fontCss,
  THEME_IDS,
  themes,
  type FontFamilyId,
} from './terminal-themes';

// Terminal.tsx has a large untestable-in-vitest surface (xterm +
// WebGL + FitAddon) but its theme and font wiring is a plain
// value lookup that we should pin. A theme rename or a default
// that falls off the list is an instant UX regression — first
// paint of every terminal binds to `themes[s.theme]`, so a missing
// key produces a black-on-black terminal at session load.

describe('terminal-themes', () => {
  it('exports a theme for every advertised id', () => {
    for (const id of THEME_IDS) {
      const theme = themes[id];
      expect(theme, `missing theme for id=${id}`).toBeDefined();
      // Xterm requires at least background + foreground; everything
      // else falls back to sensible defaults. Pin both because a
      // missing foreground produces invisible text, not a crash,
      // which is the worst kind of regression.
      expect(typeof theme.background, `${id}.background`).toBe('string');
      expect(typeof theme.foreground, `${id}.foreground`).toBe('string');
    }
  });

  it('declares a DEFAULT_THEME that exists in the themes map', () => {
    expect(themes[DEFAULT_THEME]).toBeDefined();
  });

  it('declares a DEFAULT_FONT_FAMILY that exists in FONT_FAMILIES', () => {
    expect(FONT_FAMILIES.some((f) => f.id === DEFAULT_FONT_FAMILY)).toBe(true);
  });

  it('fontCss returns the css string for a known id', () => {
    for (const f of FONT_FAMILIES) {
      expect(fontCss(f.id)).toBe(f.css);
    }
  });

  it('fontCss falls back to the first registered family for an unknown id', () => {
    // The `?? FONT_FAMILIES[0].css` fallback is the important contract:
    // a bad id must NEVER return `undefined`, because xterm will set
    // `fontFamily: undefined` which collapses the terminal font to
    // whatever the browser picks and silently breaks rendering. A
    // future refactor that switches to a `Map<string, …>` without
    // keeping the fallback will trip this test.
    expect(fontCss('does-not-exist' as FontFamilyId)).toBe(FONT_FAMILIES[0].css);
  });
});
