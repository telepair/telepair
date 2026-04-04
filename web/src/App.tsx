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
