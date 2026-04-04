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
