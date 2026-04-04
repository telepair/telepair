// web/src/stores/auth.ts
import { createSignal } from 'solid-js';
import { api, ApiError } from '../lib/api';

export const STORAGE_KEY = 'telepair_token';

const [token, setTokenSignal] = createSignal(localStorage.getItem(STORAGE_KEY) ?? '');
const [validating, setValidating] = createSignal(false);
const [error, setError] = createSignal('');

function setToken(value: string) {
  if (value) {
    localStorage.setItem(STORAGE_KEY, value);
  } else {
    localStorage.removeItem(STORAGE_KEY);
  }
  setTokenSignal(value);
  setError('');
}

async function validateToken(t: string): Promise<boolean> {
  setValidating(true);
  setError('');
  try {
    setToken(t);
    await api.listTargets();
    setValidating(false);
    return true;
  } catch (e) {
    setToken('');
    if (e instanceof ApiError && e.status === 401) {
      setError('Invalid token');
    } else {
      setError('Connection failed');
    }
    setValidating(false);
    return false;
  }
}

function logout() {
  setToken('');
}

function isAuthenticated(): boolean {
  return token().length > 0;
}

export const auth = {
  token,
  validating,
  error,
  setToken,
  validateToken,
  logout,
  isAuthenticated,
};
