// web/src/stores/session.ts
import { createSignal } from 'solid-js';
import { api, ApiError, type ListSessionsOptions } from '../lib/api';
import { auth } from './auth';
import type { TargetInfo, Session, InputMode, SessionStatus } from '../lib/protocol';

/** Tab value for the Dashboard sessions section. 'all' is distinct
 *  from 'undefined' because the UI has an explicit "All" chip — we
 *  want to serialize it to the backend as "no status filter", which
 *  `api.listSessions` already handles. */
export type SessionsFilter = SessionStatus | 'all';

const [targets, setTargets] = createSignal<TargetInfo[]>([]);
const [sessions, setSessions] = createSignal<Session[]>([]);
const [loading, setLoading] = createSignal(false);
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
  setLoading(true);
  try {
    const data = await api.listTargets();
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
    if (e instanceof ApiError && e.status === 403) {
      auth.logoutAndRedirect();
      return;
    }
    console.error('fetchTargets failed:', e);
  } finally {
    setLoading(false);
  }
}

async function fetchSessions(
  filter: SessionsFilter = currentFilter(),
  targetName: string = currentTargetFilter(),
) {
  setCurrentFilter(filter);
  setCurrentTargetFilter(targetName);
  const opts: ListSessionsOptions =
    filter === 'all' ? { status: 'all' } : { status: filter };
  if (targetName) {
    opts.targetName = targetName;
  }
  try {
    const data = await api.listSessions(opts);
    setSessions(data);
  } catch {
    // On failure, drop any rows from the previous filter. Leaving
    // them in place would render (say) active rows under the Closed
    // tab because the Closed fetch 500'd mid-switch — an empty list
    // is strictly better than showing the wrong bucket.
    setSessions([]);
  }
}

async function createSession(targetName: string, inputMode?: InputMode): Promise<Session> {
  const session = await api.createSession(targetName, inputMode);
  // Only surface newly-created sessions on tabs that would show them:
  //   1. Not the "Closed" tab — a freshly-minted row is always active.
  //   2. No target filter OR the filter matches the new session's target
  //      — if the dashboard is filtered to `?target=alpha` and the user
  //      creates a `beta` session, the new row must NOT appear in the
  //      alpha list (it would vanish on the next refetch anyway, but the
  //      flash is confusing). An empty target filter means "show all".
  const targetOk =
    currentTargetFilter() === '' || currentTargetFilter() === targetName;
  if (currentFilter() !== 'closed' && targetOk) {
    setSessions((prev) => [...prev, session]);
  }
  return session;
}

async function refresh() {
  await Promise.all([fetchTargets(), fetchSessions()]);
}

export const sessionStore = {
  targets,
  sessions,
  loading,
  currentFilter,
  currentTargetFilter,
  fetchTargets,
  fetchSessions,
  createSession,
  refresh,
};
