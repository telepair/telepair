// web/src/i18n/labels.ts
//
// Tiny helpers that turn protocol enum values into the matching
// translated short label. Centralised here so a new role/input-mode
// only needs the lookup table updated in one place — every list,
// badge, and dropdown picks the new label up automatically.
//
// Why this exists instead of inlining `t('roles.' + role)` at every
// callsite:
//   - String concatenation defeats `TranslationKey`'s typecheck — a
//     typo would silently degrade to "missing key" at runtime.
//   - Hand-written switches drift out of sync with the union (a new
//     `Role` variant would compile cleanly while quietly rendering
//     the raw enum). The `Record<Role, …>` lookup below makes that
//     impossible: adding a new variant is a tsc error here.
//   - Components using these labels (ParticipantList, Session topbar,
//     Dashboard sessions list, …) used to print the raw protocol
//     value, leaving the UI half-English under the Chinese locale.

import type { InputMode, Role } from '../lib/protocol';
import type { TranslationKey, Translator } from './provider';

const ROLE_KEYS: Record<Role, TranslationKey> = {
  owner: 'roles.owner',
  operator: 'roles.operator',
  viewer: 'roles.viewer',
};

const INPUT_MODE_KEYS: Record<InputMode, TranslationKey> = {
  multiplexed: 'input_mode.multiplexed',
  serialized: 'input_mode.serialized',
};

/** Translated short label for a participant role (Owner / Operator / Viewer). */
export function roleLabel(t: Translator, role: Role): string {
  return t(ROLE_KEYS[role]);
}

/** Translated short label for a session input mode (Collaborative / Solo). */
export function inputModeLabel(t: Translator, mode: InputMode): string {
  return t(INPUT_MODE_KEYS[mode]);
}
