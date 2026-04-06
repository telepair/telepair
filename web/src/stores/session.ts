// web/src/stores/session.ts
import { createSignal } from 'solid-js';
import { api } from '../lib/api';
import type { TargetInfo, Session } from '../lib/protocol';

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
    // rejection. Other errors (500, network) fall through to
    // targets=[] which the Dashboard renders as an empty state; that
    // is not ideal but is strictly better than the previous silent
    // swallow that masked stale-token bugs.
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

async function createSession(targetName: string): Promise<Session> {
  const session = await api.createSession(targetName);
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
