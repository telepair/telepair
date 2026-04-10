import { describe, it, expect } from 'vitest';
import { parseReloadError } from './AdminTargets';

// `parseReloadError` is the seam between the gateway's structured
// reload-failure JSON and the admin UI. It used to silently drop
// every field beyond `reason` / `message`, which made the
// `still_referenced` reload guard look like a generic toast with a
// raw blob of JSON inside it. The fix surfaces the `targets` array
// so the page can render a real list — these tests pin the parser's
// contract so a future server change (e.g. renaming a field) blows
// up here, not at integration time.

describe('parseReloadError', () => {
  it('returns null when the body is not JSON', () => {
    expect(parseReloadError('not-json')).toBeNull();
  });

  it('returns null when the body is JSON but not an object', () => {
    expect(parseReloadError('"oops"')).toBeNull();
    expect(parseReloadError('null')).toBeNull();
    expect(parseReloadError('[]')).toBeNull();
  });

  it('returns null when the reason field is missing', () => {
    // Without `reason` we cannot dispatch to a translated message,
    // so the caller MUST fall back to the generic toast — silently
    // returning a partial parse would mask the missing field and
    // make the page look "fine" while showing nothing.
    expect(parseReloadError('{"message":"x"}')).toBeNull();
  });

  it('parses the no_targets_path shape', () => {
    const parsed = parseReloadError(
      JSON.stringify({
        reason: 'no_targets_path',
        message: 'configure first',
      }),
    );
    expect(parsed).toEqual({
      reason: 'no_targets_path',
      message: 'configure first',
      targets: undefined,
    });
  });

  it('parses the parse_error shape', () => {
    const parsed = parseReloadError(
      JSON.stringify({
        reason: 'parse_error',
        message: 'line 3: expected mapping',
        path: '/etc/telepair/targets.yaml',
      }),
    );
    expect(parsed?.reason).toBe('parse_error');
    expect(parsed?.message).toBe('line 3: expected mapping');
    expect(parsed?.targets).toBeUndefined();
  });

  // The load-bearing case: the new reload guard's payload includes
  // a `targets` array of `{target, active_sessions}` rows. The page
  // renders the list as-is, so the parser MUST preserve every row
  // in order.
  it('parses still_referenced and preserves the targets array order', () => {
    const parsed = parseReloadError(
      JSON.stringify({
        reason: 'still_referenced',
        message: 'refusing to drop in-use targets',
        targets: [
          { target: 'prod-payments-db', active_sessions: 3 },
          { target: 'legacy-redis', active_sessions: 1 },
        ],
      }),
    );
    expect(parsed?.reason).toBe('still_referenced');
    expect(parsed?.targets).toEqual([
      { target: 'prod-payments-db', active_sessions: 3 },
      { target: 'legacy-redis', active_sessions: 1 },
    ]);
  });

  it('drops malformed rows from the still_referenced payload instead of crashing', () => {
    // Defensive: a future server bug shouldn't take down the
    // entire admin page. The parser keeps the well-formed rows
    // and silently discards the rest — the banner then renders
    // the survivors, which is strictly better than a `<For>` blow
    // up over an unexpected shape.
    const parsed = parseReloadError(
      JSON.stringify({
        reason: 'still_referenced',
        message: 'mixed payload',
        targets: [
          { target: 'good', active_sessions: 2 },
          { target: 'missing-count' },
          { active_sessions: 1 }, // missing target
          'string-row',
          null,
          { target: 'also-good', active_sessions: 0 },
        ],
      }),
    );
    expect(parsed?.targets).toEqual([
      { target: 'good', active_sessions: 2 },
      { target: 'also-good', active_sessions: 0 },
    ]);
  });

  it('leaves targets undefined when still_referenced ships no array', () => {
    // A degenerate server response (`reason` set, `targets`
    // missing) must not stand in for "zero blocking targets" —
    // that would render an empty banner with no actionable rows.
    // The page falls back to the generic toast in that case.
    const parsed = parseReloadError(
      JSON.stringify({ reason: 'still_referenced', message: 'nope' }),
    );
    expect(parsed?.targets).toBeUndefined();
  });
});
