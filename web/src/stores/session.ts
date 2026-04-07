// web/src/stores/session.ts
import { createSignal } from 'solid-js';
import { api, ApiError } from '../lib/api';
import { auth } from './auth';
import type { TargetInfo, Session, InputMode } from '../lib/protocol';

const [targets, setTargets] = createSignal<TargetInfo[]>([]);
const [sessions, setSessions] = createSignal<Session[]>([]);
const [loading, setLoading] = createSignal(false);

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

async function fetchSessions() {
  try {
    const data = await api.listSessions();
    setSessions(data);
  } catch {
    // ignore — dashboard still usable without sessions list
  }
}

async function createSession(targetName: string, inputMode?: InputMode): Promise<Session> {
  const session = await api.createSession(targetName, inputMode);
  setSessions((prev) => [...prev, session]);
  return session;
}

async function refresh() {
  await Promise.all([fetchTargets(), fetchSessions()]);
}

export const sessionStore = {
  targets,
  sessions,
  loading,
  fetchTargets,
  fetchSessions,
  createSession,
  refresh,
};
