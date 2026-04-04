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
