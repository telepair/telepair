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
