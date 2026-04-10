// web/src/App.tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show, onMount } from 'solid-js';
import { auth } from './stores/auth';
import { I18nProvider } from './i18n';
import Login from './pages/Login';
import Dashboard from './pages/Dashboard';
import Session from './pages/Session';
import Join from './pages/Join';
import AdminTargets from './pages/AdminTargets';
import ToastContainer from './components/Toast';

function AuthGuard(props: { children: any }) {
  return (
    <Show when={auth.isAuthenticated()} fallback={<Navigate href="/login" />}>
      {props.children}
    </Show>
  );
}

/**
 * Admin-only route gate. Sits *inside* AuthGuard so the login bounce
 * for unauthenticated callers runs first (a non-admin guest with no
 * token should see /login, not the dashboard). The admin flag is
 * three-state:
 *   - `null`: whoami hasn't landed yet — render nothing so the admin
 *     UI doesn't flash in front of a guest whose flag is about to
 *     come back `false`. The whoami call is primed on login and from
 *     the Dashboard's `onMount`; we also kick it from here so a deep
 *     link to `/admin/targets` on a fresh tab reload still resolves.
 *   - `false`: authenticated but not admin → bounce to `/`.
 *   - `true`: render the guarded content.
 *
 * Using `Navigate` inside a `Show.fallback` for the "not admin" case
 * keeps the bounce consistent with `AuthGuard` — the router swaps the
 * route instead of the component trying to `useNavigate` from a
 * mount effect, which would race with the first paint.
 */
function AdminGuard(props: { children: any }) {
  onMount(() => {
    // Fire-and-forget: loadIdentity is idempotent and swallows errors.
    // This covers the "hard reload on /admin/targets" path where the
    // Dashboard's mount hook never ran.
    auth.loadIdentity();
  });
  return (
    <Show
      when={auth.currentUserIsAdmin() !== null}
      fallback={<div />}
    >
      <Show
        when={auth.currentUserIsAdmin() === true}
        fallback={<Navigate href="/" />}
      >
        {props.children}
      </Show>
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
        <Route
          path="/admin/targets"
          component={() => (
            <AuthGuard>
              <AdminGuard>
                <AdminTargets />
              </AdminGuard>
            </AuthGuard>
          )}
        />
      </Router>
      <ToastContainer />
    </I18nProvider>
  );
}
