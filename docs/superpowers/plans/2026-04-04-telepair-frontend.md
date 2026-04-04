# Telepair Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a SolidJS + xterm.js web frontend that connects to the telepair backend, providing login, dashboard, and interactive terminal sessions.

**Architecture:** SPA with 3 pages (Login, Dashboard, Session). Auth via Bearer token stored in localStorage. Terminal I/O over WebSocket using the telepair JSON protocol. Vite dev server proxies to backend at :7700. Production build served by gateway via tower-http ServeDir.

**Tech Stack:** SolidJS 1.9, @solidjs/router 0.15, @xterm/xterm 5.5, @xterm/addon-fit 0.10, @xterm/addon-webgl 0.18, Vite 6, TypeScript 5.7, Vitest 3

---

## Dependency Graph

```
Task 1 (Scaffold) → Task 2 (Protocol Types) → Task 3 (API Client) → Task 4 (Auth + Login)
                                                      ↓
                                               Task 5 (Session Store + Dashboard)
                                                      ↓
Task 6 (WebSocket Client) ────────────────→ Task 8 (Session Page)
Task 7 (Terminal Component) ──────────────→ Task 8 (Session Page)

Task 9 (Gateway Static Serving) — independent Rust task
```

## File Structure

```
web/
├── package.json              — dependencies, scripts
├── tsconfig.json             — TypeScript config for SolidJS JSX
├── vite.config.ts            — Vite + SolidJS plugin + proxy
├── index.html                — SPA entry HTML
└── src/
    ├── index.tsx             — mount SolidJS app
    ├── App.tsx               — router + layout shell
    ├── index.css             — global dark theme styles
    ├── lib/
    │   ├── protocol.ts       — TypeScript types mirroring Rust protocol
    │   ├── api.ts            — REST API client (typed fetch wrapper)
    │   └── ws.ts             — WebSocket client (connect, send, handle messages)
    ├── stores/
    │   ├── auth.ts           — token persistence, user state, guard
    │   └── session.ts        — target list, session list, create session
    ├── pages/
    │   ├── Login.tsx         — token input form
    │   ├── Dashboard.tsx     — target grid + active sessions list
    │   └── Session.tsx       — terminal view with WebSocket
    └── components/
        └── Terminal.tsx      — xterm.js wrapper component
```

---

### Task 1: Vite + SolidJS Project Scaffold

**Files:**
- Create: `web/package.json`
- Create: `web/tsconfig.json`
- Create: `web/vite.config.ts`
- Create: `web/index.html`
- Create: `web/src/index.tsx`
- Create: `web/src/App.tsx`
- Create: `web/src/index.css`

**Depends on:** Nothing

- [ ] **Step 1: Create package.json**

```json
{
  "name": "telepair-web",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview",
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "dependencies": {
    "solid-js": "^1.9.0",
    "@solidjs/router": "^0.15.0",
    "@xterm/xterm": "^5.5.0",
    "@xterm/addon-fit": "^0.10.0",
    "@xterm/addon-webgl": "^0.18.0"
  },
  "devDependencies": {
    "vite": "^6.0.0",
    "vite-plugin-solid": "^2.10.0",
    "typescript": "^5.7.0",
    "vitest": "^3.0.0"
  }
}
```

- [ ] **Step 2: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "preserve",
    "jsxImportSource": "solid-js",
    "strict": true,
    "noEmit": true,
    "isolatedModules": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "resolveJsonModule": true
  },
  "include": ["src"]
}
```

- [ ] **Step 3: Create vite.config.ts**

```typescript
import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';

export default defineConfig({
  plugins: [solid()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:7700',
      '/ws': {
        target: 'ws://localhost:7700',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    target: 'esnext',
  },
});
```

- [ ] **Step 4: Create index.html**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>telepair</title>
</head>
<body>
  <div id="app"></div>
  <script src="/src/index.tsx" type="module"></script>
</body>
</html>
```

- [ ] **Step 5: Create src/index.css (dark theme)**

```css
:root {
  --bg-primary: #0d1117;
  --bg-secondary: #161b22;
  --bg-tertiary: #21262d;
  --border: #30363d;
  --text-primary: #e6edf3;
  --text-secondary: #8b949e;
  --accent: #58a6ff;
  --accent-hover: #79c0ff;
  --success: #3fb950;
  --error: #f85149;
  --warning: #d29922;
  --font-mono: 'Menlo', 'Monaco', 'Courier New', monospace;
  --font-sans: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif;
}

* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

body {
  font-family: var(--font-sans);
  background: var(--bg-primary);
  color: var(--text-primary);
  line-height: 1.5;
  min-height: 100vh;
}

a {
  color: var(--accent);
  text-decoration: none;
}

a:hover {
  color: var(--accent-hover);
  text-decoration: underline;
}

button {
  font-family: var(--font-sans);
  cursor: pointer;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 6px 16px;
  font-size: 14px;
  background: var(--bg-tertiary);
  color: var(--text-primary);
  transition: background 0.15s;
}

button:hover {
  background: var(--border);
}

button.primary {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}

button.primary:hover {
  background: var(--accent-hover);
  border-color: var(--accent-hover);
}

input {
  font-family: var(--font-mono);
  background: var(--bg-tertiary);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 12px;
  color: var(--text-primary);
  font-size: 14px;
  width: 100%;
}

input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 3px rgba(88, 166, 255, 0.3);
}
```

- [ ] **Step 6: Create src/index.tsx**

```tsx
import { render } from 'solid-js/web';
import App from './App';
import './index.css';

const root = document.getElementById('app');
if (!root) throw new Error('Root element #app not found');

render(() => <App />, root);
```

- [ ] **Step 7: Create src/App.tsx (minimal placeholder)**

```tsx
export default function App() {
  return (
    <div style={{ display: 'flex', 'align-items': 'center', 'justify-content': 'center', 'min-height': '100vh' }}>
      <h1 style={{ 'font-size': '24px', 'font-weight': '600' }}>telepair</h1>
    </div>
  );
}
```

- [ ] **Step 8: Install dependencies and verify**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npm install 2>&1 | tail -5`
Expected: packages installed successfully

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds, `dist/` created

- [ ] **Step 9: Add web to .gitignore and commit**

Append to `/Users/liys/workspace/github.com/telepair/telepair/.gitignore`:
```
web/node_modules/
web/dist/
```

```bash
git add web/ .gitignore
git commit -s -m "feat(web): init Vite + SolidJS project scaffold"
```

---

### Task 2: TypeScript Protocol Types

**Files:**
- Create: `web/src/lib/protocol.ts`

**Depends on:** Task 1

- [ ] **Step 1: Create protocol.ts with all types**

```typescript
// web/src/lib/protocol.ts
// TypeScript types mirroring crates/telepair-core/src/protocol.rs

export type Role = 'owner' | 'operator' | 'viewer';

export type InputMode = 'serialized' | 'multiplexed';

export type SessionStatus = 'active' | 'closed';

export interface Session {
  id: string;
  owner_id: string;
  target_name: string;
  input_mode: InputMode;
  status: SessionStatus;
  created_at: string;
  closed_at: string | null;
}

export interface ParticipantInfo {
  user_id: string;
  name: string;
  role: Role;
  color: string;
}

export interface TargetInfo {
  name: string;
  display: string;
  tags: string[];
}

// --- Client → Server ---

export type ClientMessage =
  | { type: 'SessionJoin'; session_id: string; token: string }
  | { type: 'TermInput'; data: number[] }
  | { type: 'TermResize'; cols: number; rows: number }
  | { type: 'CursorMove'; x: number; y: number }
  | { type: 'ChatMessage'; text: string };

// --- Server → Client ---

export type ServerMessage =
  | { type: 'SessionState'; session: Session; participants: ParticipantInfo[]; your_role: Role }
  | { type: 'TermOutput'; data: number[] }
  | { type: 'PeerJoined'; user_id: string; name: string; role: Role; color: string }
  | { type: 'PeerLeft'; user_id: string }
  | { type: 'PeerCursor'; user_id: string; x: number; y: number }
  | { type: 'PeerChat'; user_id: string; name: string; text: string; ts: string }
  | { type: 'PermUpdate'; user_id: string; new_role: Role }
  | { type: 'Error'; code: string; message: string };

// --- Helpers ---

export function encodeInput(text: string): number[] {
  return Array.from(new TextEncoder().encode(text));
}

export function decodeOutput(data: number[]): Uint8Array {
  return new Uint8Array(data);
}
```

- [ ] **Step 2: Commit**

```bash
git add web/src/lib/protocol.ts
git commit -s -m "feat(web): add TypeScript protocol types"
```

---

### Task 3: REST API Client

**Files:**
- Create: `web/src/lib/api.ts`

**Depends on:** Task 2

- [ ] **Step 1: Create api.ts**

```typescript
// web/src/lib/api.ts
import type { TargetInfo, Session } from './protocol';

const BASE = '/api';

class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
    this.name = 'ApiError';
  }
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const token = localStorage.getItem('telepair_token');
  const headers: Record<string, string> = {
    ...options.headers as Record<string, string>,
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }
  if (options.body && typeof options.body === 'string') {
    headers['Content-Type'] = 'application/json';
  }

  const resp = await fetch(`${BASE}${path}`, { ...options, headers });

  if (!resp.ok) {
    throw new ApiError(resp.status, await resp.text());
  }

  return resp.json();
}

export const api = {
  health(): Promise<{ status: string }> {
    return request('/health');
  },

  listTargets(): Promise<TargetInfo[]> {
    return request('/targets');
  },

  listSessions(): Promise<Session[]> {
    return request('/sessions');
  },

  createSession(target_name: string, input_mode?: string): Promise<Session> {
    return request('/sessions', {
      method: 'POST',
      body: JSON.stringify({ target_name, input_mode }),
    });
  },
};

export { ApiError };
```

- [ ] **Step 2: Commit**

```bash
git add web/src/lib/api.ts
git commit -s -m "feat(web): add REST API client"
```

---

### Task 4: Auth Store + Login Page

**Files:**
- Create: `web/src/stores/auth.ts`
- Create: `web/src/pages/Login.tsx`
- Modify: `web/src/App.tsx`

**Depends on:** Task 3

- [ ] **Step 1: Create stores/auth.ts**

```typescript
// web/src/stores/auth.ts
import { createSignal } from 'solid-js';
import { api, ApiError } from '../lib/api';

const STORAGE_KEY = 'telepair_token';

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
    localStorage.setItem(STORAGE_KEY, t);
    await api.listTargets();
    setTokenSignal(t);
    setValidating(false);
    return true;
  } catch (e) {
    localStorage.removeItem(STORAGE_KEY);
    setTokenSignal('');
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
```

- [ ] **Step 2: Create pages/Login.tsx**

```tsx
// web/src/pages/Login.tsx
import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';

export default function Login() {
  const navigate = useNavigate();
  const [input, setInput] = createSignal('');

  const handleSubmit = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.validateToken(input());
    if (ok) {
      navigate('/', { replace: true });
    }
  };

  return (
    <div class="login-page">
      <div class="login-card">
        <h1>telepair</h1>
        <p class="subtitle">Web terminal collaboration</p>

        <form onSubmit={handleSubmit}>
          <label for="token">API Token</label>
          <input
            id="token"
            type="password"
            placeholder="Paste your token here"
            value={input()}
            onInput={(e) => setInput(e.currentTarget.value)}
            autofocus
          />

          <Show when={auth.error()}>
            <p class="error-msg">{auth.error()}</p>
          </Show>

          <button type="submit" class="primary" disabled={auth.validating() || !input()}>
            {auth.validating() ? 'Validating...' : 'Connect'}
          </button>
        </form>
      </div>

      <style>{`
        .login-page {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
        }
        .login-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 12px;
          padding: 40px;
          width: 380px;
          text-align: center;
        }
        .login-card h1 {
          font-size: 28px;
          font-weight: 700;
          margin-bottom: 4px;
        }
        .subtitle {
          color: var(--text-secondary);
          margin-bottom: 24px;
          font-size: 14px;
        }
        .login-card form {
          text-align: left;
        }
        .login-card label {
          display: block;
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          margin-bottom: 6px;
        }
        .login-card input {
          margin-bottom: 16px;
        }
        .login-card button {
          width: 100%;
          padding: 10px;
          font-size: 15px;
        }
        .error-msg {
          color: var(--error);
          font-size: 13px;
          margin-bottom: 12px;
        }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 3: Update App.tsx with router and auth guard**

```tsx
// web/src/App.tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show } from 'solid-js';
import { auth } from './stores/auth';
import Login from './pages/Login';

function AuthGuard(props: { children: any }) {
  return (
    <Show when={auth.isAuthenticated()} fallback={<Navigate href="/login" />}>
      {props.children}
    </Show>
  );
}

function DashboardPlaceholder() {
  return (
    <AuthGuard>
      <div style={{ padding: '40px', 'text-align': 'center' }}>
        <h2>Dashboard</h2>
        <p style={{ color: 'var(--text-secondary)' }}>Coming in Task 5</p>
        <button onClick={() => auth.logout()} style={{ 'margin-top': '16px' }}>Logout</button>
      </div>
    </AuthGuard>
  );
}

export default function App() {
  return (
    <Router>
      <Route path="/login" component={Login} />
      <Route path="/" component={DashboardPlaceholder} />
    </Router>
  );
}
```

- [ ] **Step 4: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds

- [ ] **Step 5: Commit**

```bash
git add web/src/
git commit -s -m "feat(web): add auth store and login page"
```

---

### Task 5: Session Store + Dashboard Page

**Files:**
- Create: `web/src/stores/session.ts`
- Create: `web/src/pages/Dashboard.tsx`
- Modify: `web/src/App.tsx`

**Depends on:** Tasks 3, 4

- [ ] **Step 1: Create stores/session.ts**

```typescript
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
```

- [ ] **Step 2: Create pages/Dashboard.tsx**

```tsx
// web/src/pages/Dashboard.tsx
import { onMount, Show, For } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { sessionStore } from '../stores/session';

export default function Dashboard() {
  const navigate = useNavigate();

  onMount(() => {
    sessionStore.refresh();
  });

  const handleLaunch = async (targetName: string) => {
    try {
      const session = await sessionStore.createSession(targetName);
      navigate(`/session/${session.id}`);
    } catch (e) {
      console.error('Failed to create session:', e);
    }
  };

  return (
    <div class="dashboard">
      <header class="topbar">
        <h1>telepair</h1>
        <button onClick={() => auth.logout()}>Logout</button>
      </header>

      <main class="content">
        <section>
          <h2>Targets</h2>
          <Show when={!sessionStore.loading()} fallback={<p class="muted">Loading...</p>}>
            <div class="target-grid">
              <For each={sessionStore.targets()} fallback={<p class="muted">No targets configured</p>}>
                {(target) => (
                  <div class="target-card" onClick={() => handleLaunch(target.name)}>
                    <div class="target-name">{target.display}</div>
                    <div class="target-id">{target.name}</div>
                    <Show when={target.tags.length > 0}>
                      <div class="tags">
                        <For each={target.tags}>
                          {(tag) => <span class="tag">{tag}</span>}
                        </For>
                      </div>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </section>

        <section>
          <h2>Active Sessions</h2>
          <Show when={sessionStore.sessions().length > 0} fallback={<p class="muted">No active sessions</p>}>
            <div class="session-list">
              <For each={sessionStore.sessions()}>
                {(session) => (
                  <div class="session-row" onClick={() => navigate(`/session/${session.id}`)}>
                    <span class="session-id">{session.id}</span>
                    <span class="session-target">{session.target_name}</span>
                    <span class="session-mode">{session.input_mode}</span>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </section>
      </main>

      <style>{`
        .dashboard { min-height: 100vh; }
        .topbar {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 12px 24px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
        }
        .topbar h1 { font-size: 18px; font-weight: 700; }
        .content { padding: 24px; max-width: 960px; margin: 0 auto; }
        .content h2 { font-size: 16px; font-weight: 600; margin-bottom: 12px; color: var(--text-secondary); }
        .content section { margin-bottom: 32px; }
        .muted { color: var(--text-secondary); font-size: 14px; }

        .target-grid {
          display: grid;
          grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
          gap: 12px;
        }
        .target-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 8px;
          padding: 16px;
          cursor: pointer;
          transition: border-color 0.15s;
        }
        .target-card:hover { border-color: var(--accent); }
        .target-name { font-weight: 600; margin-bottom: 4px; }
        .target-id { font-family: var(--font-mono); font-size: 12px; color: var(--text-secondary); }
        .tags { margin-top: 8px; display: flex; gap: 4px; flex-wrap: wrap; }
        .tag {
          font-size: 11px;
          padding: 2px 8px;
          border-radius: 12px;
          background: var(--bg-tertiary);
          color: var(--text-secondary);
        }

        .session-list { display: flex; flex-direction: column; gap: 4px; }
        .session-row {
          display: flex;
          gap: 16px;
          padding: 10px 14px;
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 6px;
          cursor: pointer;
          font-size: 14px;
          transition: border-color 0.15s;
        }
        .session-row:hover { border-color: var(--accent); }
        .session-id { font-family: var(--font-mono); color: var(--accent); min-width: 100px; }
        .session-target { color: var(--text-primary); }
        .session-mode { color: var(--text-secondary); margin-left: auto; font-size: 12px; }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 3: Update App.tsx with Dashboard route**

```tsx
// web/src/App.tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show } from 'solid-js';
import { auth } from './stores/auth';
import Login from './pages/Login';
import Dashboard from './pages/Dashboard';

function AuthGuard(props: { children: any }) {
  return (
    <Show when={auth.isAuthenticated()} fallback={<Navigate href="/login" />}>
      {props.children}
    </Show>
  );
}

function SessionPlaceholder() {
  return (
    <AuthGuard>
      <div style={{ padding: '40px', 'text-align': 'center' }}>
        <h2>Session</h2>
        <p style={{ color: 'var(--text-secondary)' }}>Coming in Task 8</p>
      </div>
    </AuthGuard>
  );
}

export default function App() {
  return (
    <Router>
      <Route path="/login" component={Login} />
      <Route path="/" component={() => <AuthGuard><Dashboard /></AuthGuard>} />
      <Route path="/session/:id" component={SessionPlaceholder} />
    </Router>
  );
}
```

- [ ] **Step 4: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds

- [ ] **Step 5: Commit**

```bash
git add web/src/
git commit -s -m "feat(web): add session store and dashboard page"
```

---

### Task 6: WebSocket Client

**Files:**
- Create: `web/src/lib/ws.ts`

**Depends on:** Task 2

- [ ] **Step 1: Create lib/ws.ts**

```typescript
// web/src/lib/ws.ts
import type { ClientMessage, ServerMessage } from './protocol';

export type MessageHandler = (msg: ServerMessage) => void;
export type StatusHandler = (status: 'connecting' | 'connected' | 'disconnected' | 'error') => void;

export class TelepairSocket {
  private ws: WebSocket | null = null;
  private onMessage: MessageHandler;
  private onStatus: StatusHandler;

  constructor(onMessage: MessageHandler, onStatus: StatusHandler) {
    this.onMessage = onMessage;
    this.onStatus = onStatus;
  }

  connect(sessionId: string, token: string) {
    this.onStatus('connecting');

    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${protocol}//${location.host}/ws/session/${sessionId}`;
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.send({
        type: 'SessionJoin',
        session_id: sessionId,
        token,
      });
    };

    this.ws.onmessage = (event) => {
      try {
        const msg: ServerMessage = JSON.parse(event.data);
        if (msg.type === 'SessionState') {
          this.onStatus('connected');
        }
        this.onMessage(msg);
      } catch {
        console.error('Failed to parse WS message:', event.data);
      }
    };

    this.ws.onclose = () => {
      this.onStatus('disconnected');
    };

    this.ws.onerror = () => {
      this.onStatus('error');
    };
  }

  send(msg: ClientMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  sendInput(data: number[]) {
    this.send({ type: 'TermInput', data });
  }

  sendResize(cols: number, rows: number) {
    this.send({ type: 'TermResize', cols, rows });
  }

  disconnect() {
    this.ws?.close();
    this.ws = null;
  }
}
```

- [ ] **Step 2: Commit**

```bash
git add web/src/lib/ws.ts
git commit -s -m "feat(web): add WebSocket client"
```

---

### Task 7: Terminal Component

**Files:**
- Create: `web/src/components/Terminal.tsx`

**Depends on:** Task 1

- [ ] **Step 1: Create components/Terminal.tsx**

```tsx
// web/src/components/Terminal.tsx
import { onMount, onCleanup } from 'solid-js';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';

export interface TerminalHandle {
  write(data: string | Uint8Array): void;
  focus(): void;
  dispose(): void;
}

interface TerminalProps {
  onData: (data: string) => void;
  onResize: (cols: number, rows: number) => void;
  ref?: (handle: TerminalHandle) => void;
}

export default function Terminal(props: TerminalProps) {
  let containerRef: HTMLDivElement | undefined;
  let term: XTerm | undefined;
  let fitAddon: FitAddon | undefined;
  let resizeObserver: ResizeObserver | undefined;

  onMount(() => {
    if (!containerRef) return;

    term = new XTerm({
      cursorBlink: true,
      cursorStyle: 'block',
      fontSize: 14,
      fontFamily: "'Menlo', 'Monaco', 'Courier New', monospace",
      scrollback: 10000,
      theme: {
        background: '#0d1117',
        foreground: '#e6edf3',
        cursor: '#e6edf3',
        selectionBackground: 'rgba(88, 166, 255, 0.3)',
        black: '#484f58',
        red: '#ff7b72',
        green: '#3fb950',
        yellow: '#d29922',
        blue: '#58a6ff',
        magenta: '#bc8cff',
        cyan: '#39c5cf',
        white: '#b1bac4',
        brightBlack: '#6e7681',
        brightRed: '#ffa198',
        brightGreen: '#56d364',
        brightYellow: '#e3b341',
        brightBlue: '#79c0ff',
        brightMagenta: '#d2a8ff',
        brightCyan: '#56d4dd',
        brightWhite: '#f0f6fc',
      },
    });

    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef);

    // Try WebGL renderer for performance
    try {
      term.loadAddon(new WebglAddon());
    } catch {
      // WebGL not available, fall back to canvas
    }

    fitAddon.fit();

    // Forward user input
    term.onData((data) => props.onData(data));

    // Forward resize
    term.onResize(({ cols, rows }) => props.onResize(cols, rows));

    // Auto-fit on container resize
    resizeObserver = new ResizeObserver(() => {
      fitAddon?.fit();
    });
    resizeObserver.observe(containerRef);

    // Expose handle to parent
    props.ref?.({
      write(data: string | Uint8Array) {
        term?.write(data);
      },
      focus() {
        term?.focus();
      },
      dispose() {
        term?.dispose();
      },
    });

    term.focus();
  });

  onCleanup(() => {
    resizeObserver?.disconnect();
    term?.dispose();
  });

  return (
    <div
      ref={containerRef}
      style={{ width: '100%', height: '100%', overflow: 'hidden' }}
    />
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds

- [ ] **Step 3: Commit**

```bash
git add web/src/components/
git commit -s -m "feat(web): add xterm.js terminal component"
```

---

### Task 8: Session Page (Full Integration)

**Files:**
- Create: `web/src/pages/Session.tsx`
- Modify: `web/src/App.tsx`

**Depends on:** Tasks 5, 6, 7

- [ ] **Step 1: Create pages/Session.tsx**

```tsx
// web/src/pages/Session.tsx
import { createSignal, onCleanup, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { TelepairSocket } from '../lib/ws';
import { encodeInput, decodeOutput } from '../lib/protocol';
import type { ServerMessage, Role } from '../lib/protocol';
import type { TerminalHandle } from '../components/Terminal';
import Terminal from '../components/Terminal';

type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

export default function SessionPage() {
  const params = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [status, setStatus] = createSignal<ConnectionStatus>('connecting');
  const [role, setRole] = createSignal<Role>('viewer');
  const [errorMsg, setErrorMsg] = createSignal('');

  let termHandle: TerminalHandle | undefined;
  let socket: TelepairSocket | undefined;

  const handleMessage = (msg: ServerMessage) => {
    switch (msg.type) {
      case 'SessionState':
        setRole(msg.your_role);
        break;
      case 'TermOutput':
        termHandle?.write(decodeOutput(msg.data));
        break;
      case 'Error':
        setErrorMsg(`${msg.code}: ${msg.message}`);
        break;
    }
  };

  const handleStatus = (s: ConnectionStatus) => {
    setStatus(s);
  };

  const handleData = (data: string) => {
    if (role() === 'viewer') return;
    socket?.sendInput(encodeInput(data));
  };

  const handleResize = (cols: number, rows: number) => {
    if (role() === 'viewer') return;
    socket?.sendResize(cols, rows);
  };

  // Connect WebSocket
  socket = new TelepairSocket(handleMessage, handleStatus);
  socket.connect(params.id, auth.token());

  onCleanup(() => {
    socket?.disconnect();
  });

  return (
    <div class="session-page">
      <header class="session-topbar">
        <button class="back-btn" onClick={() => navigate('/')}>← Back</button>
        <span class="session-label">Session: <code>{params.id}</code></span>
        <span class="role-badge" data-role={role()}>{role()}</span>
        <span class="status-dot" data-status={status()} />
      </header>

      <Show when={errorMsg()}>
        <div class="error-banner">{errorMsg()}</div>
      </Show>

      <div class="terminal-container">
        <Terminal
          onData={handleData}
          onResize={handleResize}
          ref={(h) => { termHandle = h; }}
        />
      </div>

      <style>{`
        .session-page {
          display: flex;
          flex-direction: column;
          height: 100vh;
        }
        .session-topbar {
          display: flex;
          align-items: center;
          gap: 12px;
          padding: 8px 16px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
          font-size: 13px;
        }
        .back-btn {
          font-size: 13px;
          padding: 4px 10px;
        }
        .session-label code {
          font-family: var(--font-mono);
          color: var(--accent);
        }
        .role-badge {
          padding: 2px 8px;
          border-radius: 12px;
          font-size: 11px;
          font-weight: 600;
          text-transform: uppercase;
        }
        .role-badge[data-role="owner"] { background: rgba(63, 185, 80, 0.2); color: var(--success); }
        .role-badge[data-role="operator"] { background: rgba(88, 166, 255, 0.2); color: var(--accent); }
        .role-badge[data-role="viewer"] { background: rgba(139, 148, 158, 0.2); color: var(--text-secondary); }

        .status-dot {
          width: 8px;
          height: 8px;
          border-radius: 50%;
          margin-left: auto;
        }
        .status-dot[data-status="connecting"] { background: var(--warning); }
        .status-dot[data-status="connected"] { background: var(--success); }
        .status-dot[data-status="disconnected"] { background: var(--text-secondary); }
        .status-dot[data-status="error"] { background: var(--error); }

        .error-banner {
          padding: 8px 16px;
          background: rgba(248, 81, 73, 0.15);
          color: var(--error);
          font-size: 13px;
          border-bottom: 1px solid rgba(248, 81, 73, 0.3);
        }

        .terminal-container {
          flex: 1;
          padding: 4px;
          overflow: hidden;
        }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 2: Update App.tsx with final routing**

```tsx
// web/src/App.tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show } from 'solid-js';
import { auth } from './stores/auth';
import Login from './pages/Login';
import Dashboard from './pages/Dashboard';
import Session from './pages/Session';

function AuthGuard(props: { children: any }) {
  return (
    <Show when={auth.isAuthenticated()} fallback={<Navigate href="/login" />}>
      {props.children}
    </Show>
  );
}

export default function App() {
  return (
    <Router>
      <Route path="/login" component={Login} />
      <Route path="/" component={() => <AuthGuard><Dashboard /></AuthGuard>} />
      <Route path="/session/:id" component={() => <AuthGuard><Session /></AuthGuard>} />
    </Router>
  );
}
```

- [ ] **Step 3: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds

- [ ] **Step 4: Commit**

```bash
git add web/src/
git commit -s -m "feat(web): add session page with terminal and WebSocket integration"
```

---

### Task 9: Gateway Static File Serving

**Files:**
- Modify: `crates/telepair-gateway/src/lib.rs`
- Modify: `crates/telepair-gateway/Cargo.toml` (if needed)

**Depends on:** Task 8

This task adds static file serving to the gateway so the built frontend is accessible at the server root.

- [ ] **Step 1: Update gateway lib.rs to serve static files**

The gateway already depends on `tower-http` with the `"fs"` feature. Add a fallback service that serves `web/dist/` and falls back to `index.html` for SPA routing.

```rust
// crates/telepair-gateway/src/lib.rs
#![deny(unsafe_code)]

pub mod http;
pub mod session_hub;
pub mod state;
pub mod ws;

use axum::{routing::{get, post}, Router};
use state::AppState;
use tower_http::services::{ServeDir, ServeFile};

pub fn build_router(state: AppState) -> Router {
    build_router_with_web_dir(state, None)
}

pub fn build_router_with_web_dir(state: AppState, web_dir: Option<&str>) -> Router {
    let api = Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route("/api/sessions", post(http::create_session).get(http::list_sessions))
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .with_state(state);

    match web_dir {
        Some(dir) => {
            let serve = ServeDir::new(dir)
                .not_found_service(ServeFile::new(format!("{dir}/index.html")));
            api.fallback_service(serve)
        }
        None => api,
    }
}
```

- [ ] **Step 2: Update CLI main.rs to pass web dir**

Modify `crates/telepair-cli/src/main.rs`:

Add a `--web-dir` CLI flag and pass it to `build_router_with_web_dir`:

```rust
// Add to Cli struct:
    /// Path to web frontend dist directory
    #[arg(long)]
    web_dir: Option<PathBuf>,
```

Replace the router construction:

```rust
    if gateway {
        let web_dir = cli.web_dir.as_ref().map(|p| p.to_str().unwrap());
        let state = AppState::new(storage, engine).await;
        let router = telepair_gateway::build_router_with_web_dir(state, web_dir);
        let addr = format!("{}:{}", cli.host, cli.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!("telepair listening on http://{addr}");
        if let Some(dir) = &cli.web_dir {
            tracing::info!("serving web frontend from {}", dir.display());
        }
        axum::serve(listener, router).await?;
    }
```

- [ ] **Step 3: Verify existing tests still pass**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS (existing tests use `build_router` which passes `None` for web_dir)

- [ ] **Step 4: Build frontend and test serving**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npm run build 2>&1 | cat`
Expected: `web/dist/` created with index.html and assets

Manual smoke test (optional):
```bash
cd /Users/liys/workspace/github.com/telepair/telepair
cargo run -- --port 7711 --web-dir web/dist &
sleep 2
curl -s http://localhost:7711/ | head -5
# Expected: HTML content with <div id="app">
kill %1
```

- [ ] **Step 5: Commit**

```bash
git add crates/telepair-gateway/ crates/telepair-cli/
git commit -s -m "feat(gateway): add static file serving for web frontend"
```

---

## Summary

After completing all 9 tasks, you will have:

1. **A SolidJS web frontend** with dark theme, 3 pages (Login, Dashboard, Session)
2. **Token-based auth** with localStorage persistence
3. **REST API integration** for targets and sessions
4. **WebSocket terminal** with xterm.js, bidirectional I/O
5. **Gateway serving** built frontend from `web/dist/`
6. **Dev workflow**: `cargo run` (backend :7700) + `cd web && npm run dev` (frontend :5173 with proxy)
7. **Production workflow**: `cd web && npm run build` then `cargo run -- --web-dir web/dist`

**What's NOT included (deferred to Plan 3):**
- Multi-user collaboration (cursors, chat, presence)
- ParticipantList, ChatPanel, InviteDialog, CollabOverlay components
- WebRTC DataChannel for terminal I/O
- Permission enforcement UI (promote/demote/kick)
