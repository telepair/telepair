// web/src/stores/session.ts
import { createSignal } from 'solid-js';
import { api, ApiError, type ListSessionsOptions } from '../lib/api';
import { auth, onTokenChange } from './auth';
import type { TargetInfo, Session, InputMode, SessionStatus } from '../lib/protocol';

/** Tab value for the Dashboard sessions section. 'all' is distinct
 *  from 'undefined' because the UI has an explicit "All" chip — we
 *  want to serialize it to the backend as "no status filter", which
 *  `api.listSessions` already handles. */
export type SessionsFilter = SessionStatus | 'all';

/**
 * Thrown by mutators (currently `createSession`) when the bearer token
 * changed between request dispatch and response settlement. Callers
 * should treat this as a cancellation, not a user-visible failure:
 * swallow silently and avoid acting on any post-`await` state, because
 * that state was computed under a superseded identity. The server-side
 * mutation itself DID land — a stranded row under the previous account
 * is the accepted trade-off for not threading AbortControllers through
 * every store method.
 */
export class AuthChangedError extends Error {
  constructor() {
    super('auth changed mid-request');
    this.name = 'AuthChangedError';
  }
}

const SESSION_PAGE_SIZE = 50;

const [targets, setTargets] = createSignal<TargetInfo[]>([]);
const [sessions, setSessions] = createSignal<Session[]>([]);
const [loading, setLoading] = createSignal(false);
const [hasMoreSessions, setHasMoreSessions] = createSignal(true);
const [loadingMore, setLoadingMore] = createSignal(false);
// Monotonic counter incremented by fetchSessions (full refresh).
// loadMoreSessions snapshots it before the request; if fetchSessions
// fires while the request is in flight, the counter advances and the
// stale append is silently discarded.
let fetchGeneration = 0;
// Separate counter bumped only by `reset()` (logout / account swap).
// Every fetcher captures it at start and short-circuits on mismatch
// so a response that arrives AFTER a token change cannot leak the
// previous identity's data into the post-reset store. Kept distinct
// from `fetchGeneration` so refresh()'s parallel fetches do not
// mistakenly invalidate each other.
let resetEpoch = 0;
// The filter the last refresh ran with. Exposed so the Dashboard can
// reflect the active tab without threading it through component props
// — switching tabs calls `fetchSessions(nextFilter)` and the tab
// highlight follows this signal.
const [currentFilter, setCurrentFilter] = createSignal<SessionsFilter>('active');
// Optional target-name filter applied on top of the status filter.
// Empty string = no target filter. Populated by the Dashboard when
// the URL carries `?target=<name>` — typically a deep link from the
// admin targets page clicking "N active sessions" on a target card.
// Kept as its own signal (not folded into `currentFilter`) so the tab
// row and the target chip can render independently.
const [currentTargetFilter, setCurrentTargetFilter] = createSignal('');

async function fetchTargets() {
  const epoch = resetEpoch;
  setLoading(true);
  try {
    const data = await api.listTargets();
    // If a reset (logout/token swap) bumped the epoch while this
    // request was in flight, the response is for a previous identity
    // — drop it silently rather than leaking targets across accounts.
    if (epoch !== resetEpoch) return;
    setTargets(data);
  } catch (e) {
    // A 401 means the cached token is stale — the api layer's
    // interceptor has already scheduled a redirect to /login, so we
    // just need to avoid propagating the error as an unhandled
    // rejection.
    //
    // A 403 on `/targets` means a scoped-guest token reached the
    // dashboard. The server (`require_unscoped` in
    // `crates/telepair-gateway/src/http.rs::list_targets`) refuses
    // to enumerate targets to guests by design — but the old code
    // swallowed the error and let the dashboard render its
    // operator-flavored empty state ("Define named commands in
    // ~/.telepair/targets.yaml..."), which both stranded the guest
    // on a UI they have no purpose on AND leaked a server-side
    // config-file path. The fix: clear credentials and bounce to
    // /login. The guest can re-redeem their invite link to get
    // back to their session — or open a fresh one if it's gone.
    //
    // Other errors (500, network) fall through to targets=[] which
    // the Dashboard renders as an empty state; that is not ideal
    // but is strictly better than masking stale-token bugs.
    if (epoch !== resetEpoch) return;
    if (e instanceof ApiError && e.status === 403) {
      auth.logoutAndRedirect();
      return;
    }
    console.error('fetchTargets failed:', e);
  } finally {
    if (epoch === resetEpoch) setLoading(false);
  }
}

function buildSessionOpts(offset: number): ListSessionsOptions {
  const filter = currentFilter();
  const opts: ListSessionsOptions =
    filter === 'all' ? { status: 'all' } : { status: filter };
  const target = currentTargetFilter();
  if (target) opts.targetName = target;
  opts.limit = SESSION_PAGE_SIZE;
  opts.offset = offset;
  return opts;
}

async function fetchSessions(
  filter: SessionsFilter = currentFilter(),
  targetName: string = currentTargetFilter(),
) {
  const epoch = resetEpoch;
  const gen = ++fetchGeneration;
  setCurrentFilter(filter);
  setCurrentTargetFilter(targetName);
  try {
    const data = await api.listSessions(buildSessionOpts(0));
    // Two stale-guards: a newer fetchSessions bumped `fetchGeneration`
    // (tab switch), or a reset bumped `resetEpoch` (logout mid-fetch).
    // Either way discard — the rows are no longer relevant.
    if (epoch !== resetEpoch || gen !== fetchGeneration) return;
    setSessions(data);
    setHasMoreSessions(data.length >= SESSION_PAGE_SIZE);
  } catch {
    if (epoch !== resetEpoch || gen !== fetchGeneration) return;
    // On failure, drop any rows from the previous filter. Leaving
    // them in place would render (say) active rows under the Closed
    // tab because the Closed fetch 500'd mid-switch — an empty list
    // is strictly better than showing the wrong bucket.
    setSessions([]);
    setHasMoreSessions(false);
  }
}

async function loadMoreSessions() {
  if (loadingMore()) return;
  setLoadingMore(true);
  const epoch = resetEpoch;
  const gen = fetchGeneration;
  try {
    const data = await api.listSessions(buildSessionOpts(sessions().length));
    // Discard on either: (1) a full refresh superseded us via
    // fetchGeneration, or (2) a reset wiped the store via resetEpoch.
    if (epoch !== resetEpoch || gen !== fetchGeneration) return;
    setSessions((prev) => [...prev, ...data]);
    setHasMoreSessions(data.length >= SESSION_PAGE_SIZE);
  } catch {
    if (epoch !== resetEpoch || gen !== fetchGeneration) return;
    setHasMoreSessions(false);
  } finally {
    if (epoch === resetEpoch) setLoadingMore(false);
  }
}

async function createSession(target: TargetInfo, inputMode?: InputMode): Promise<Session> {
  const epoch = resetEpoch;
  const session = await api.createSession(target, inputMode);
  // If a token swap invalidated our identity mid-request, the returned
  // row belongs to the previous account. Throw `AuthChangedError` so
  // callers (Dashboard) cannot navigate the NEW identity into a session
  // created by the OLD one — returning the `Session` here would let
  // `navigate(`/session/${id}`)` cross-account leak. The server row is
  // stranded under the previous account; see the class docstring.
  if (epoch !== resetEpoch) throw new AuthChangedError();
  // Only surface newly-created sessions on tabs that would show them:
  //   1. Not the "Closed" tab — a freshly-minted row is always active.
  //   2. No target filter OR the filter matches the new session's target
  //      — if the dashboard is filtered to `?target=alpha` and the user
  //      creates a `beta` session, the new row must NOT appear in the
  //      alpha list (it would vanish on the next refetch anyway, but the
  //      flash is confusing). An empty target filter means "show all".
  const targetOk =
    currentTargetFilter() === '' || currentTargetFilter() === target.name;
  if (currentFilter() !== 'closed' && targetOk) {
    setSessions((prev) => [...prev, session]);
  }
  return session;
}

async function refresh() {
  await Promise.all([fetchTargets(), fetchSessions()]);
}

/**
 * Drop all cached state and invalidate any in-flight requests.
 * Called automatically on token change (login/logout/account swap)
 * via the `onTokenChange` subscription below, and exposed on the
 * store for tests. Synchronous — the next Dashboard mount sees an
 * empty list rather than the previous identity's rows for a frame.
 */
function reset() {
  // Bump the reset epoch first so every in-flight fetcher discards
  // its apply on settlement. `fetchGeneration` intentionally stays
  // untouched — it is for pagination ordering (fetchSessions vs
  // loadMoreSessions), not identity scoping.
  resetEpoch++;
  setTargets([]);
  setSessions([]);
  setHasMoreSessions(true);
  setLoadingMore(false);
  setLoading(false);
  setCurrentFilter('active');
  setCurrentTargetFilter('');
}

// Register at module init. Subsequent imports of this module share the
// singleton signals, so the listener is installed exactly once for the
// tab's lifetime. Any token value change (login, logout, guest invite
// redemption, admin→guest→admin in the same tab) triggers a reset.
onTokenChange(() => {
  reset();
});

export const sessionStore = {
  targets,
  sessions,
  loading,
  loadingMore,
  hasMoreSessions,
  currentFilter,
  currentTargetFilter,
  fetchTargets,
  fetchSessions,
  loadMoreSessions,
  createSession,
  refresh,
  reset,
};
