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
 * token should see /login, not the dashboard).
 *
 * Blocks rendering until `auth.identityChecked()` flips — set by
 * `loadIdentity()` once its whoami settles (success OR failure). The
 * three-state `currentUserIsAdmin()` was the old guard condition, but
 * it conflated "still fetching" and "fetched and found nothing", so a
 * transient whoami failure would strand the user on a blank `<div />`
 * forever with no recovery path.
 *
 * After the check settles:
 *   - `isAdmin === true` → render children.
 *   - anything else (false, null from a failed whoami) → bounce to
 *     `/`. The dashboard's `onMount` will retry `loadIdentity` so the
 *     user gets another chance without being forced through /login.
 */
function AdminGuard(props: { children: any }) {
  onMount(() => {
    // Fire-and-forget: loadIdentity is idempotent and de-duplicated.
    // Covers the "hard reload on /admin/targets" path where the
    // Dashboard's mount hook never ran. On success it sets both
    // `currentUser` and `identityChecked`; on failure it only sets
    // `identityChecked`, which triggers the redirect below instead
    // of leaving the page blank indefinitely.
    auth.loadIdentity();
  });
  return (
    <Show when={auth.identityChecked()} fallback={<div />}>
      <Show when={auth.currentUserIsAdmin() === true} fallback={<Navigate href="/" />}>
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
