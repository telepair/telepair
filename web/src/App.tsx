// web/src/App.tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show } from 'solid-js';
import { auth } from './stores/auth';
import { I18nProvider } from './i18n';
import Login from './pages/Login';
import Dashboard from './pages/Dashboard';
import Session from './pages/Session';
import Join from './pages/Join';
import ToastContainer from './components/Toast';

function AuthGuard(props: { children: any }) {
  return (
    <Show when={auth.isAuthenticated()} fallback={<Navigate href="/login" />}>
      {props.children}
    </Show>
  );
}

export default function App() {
  // I18nProvider must wrap the Router (not the other way around) so the
  // locale signal lives for the lifetime of the app and is not recreated
  // on every route change. ToastContainer also needs i18n for its
  // aria-label, so it lives inside the provider too.
  return (
    <I18nProvider>
      <Router>
        <Route path="/login" component={Login} />
        <Route path="/join/:token" component={Join} />
        <Route path="/" component={() => <AuthGuard><Dashboard /></AuthGuard>} />
        <Route path="/session/:id" component={() => <AuthGuard><Session /></AuthGuard>} />
      </Router>
      <ToastContainer />
    </I18nProvider>
  );
}
