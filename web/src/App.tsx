// web/src/App.tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show, onMount, lazy, type JSX } from 'solid-js';
import { auth } from './stores/auth';
import { I18nProvider } from './i18n';
// Login and Register stay eager: they're the entry for unauthenticated
// users and the SPA shell cannot paint anything useful without one of
// them already parsed. Everything else is lazy so the first-paint bundle
// no longer drags xterm.js, the admin tables, or the recording player
// into every visit. SolidJS Router accepts `lazy()` components directly
// as `component=` values — Vite/Rolldown splits each into its own chunk.
import Login from './pages/Login';
import Register from './pages/Register';
const Dashboard = lazy(() => import('./pages/Dashboard'));
const Session = lazy(() => import('./pages/Session'));
const Join = lazy(() => import('./pages/Join'));
const AdminTargets = lazy(() => import('./pages/AdminTargets'));
const AdminUsers = lazy(() => import('./pages/AdminUsers'));
const AdminAudit = lazy(() => import('./pages/AdminAudit'));
const AdminSystem = lazy(() => import('./pages/AdminSystem'));
const ChangePassword = lazy(() => import('./pages/ChangePassword'));
const Recordings = lazy(() => import('./pages/Recordings'));
const RecordingPlayer = lazy(() => import('./pages/RecordingPlayer'));
import ToastContainer from './components/Toast';

function AuthGuard(props: { children: JSX.Element }) {
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
function AdminGuard(props: { children: JSX.Element }) {
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
        <Route path="/register" component={Register} />
        <Route path="/join/:token" component={Join} />
        <Route path="/" component={() => <AuthGuard><Dashboard /></AuthGuard>} />
        <Route path="/session/:id" component={() => <AuthGuard><Session /></AuthGuard>} />
        <Route path="/change-password" component={() => <AuthGuard><ChangePassword /></AuthGuard>} />
        <Route path="/recordings" component={() => <AuthGuard><Recordings /></AuthGuard>} />
        <Route path="/recordings/:id" component={() => <AuthGuard><RecordingPlayer /></AuthGuard>} />
        {/*
          Anonymous share-token playback. Lives outside AuthGuard so a
          recipient with a `#token=...` URL fragment can open the link
          without an account; the player component reads the fragment,
          scrubs it via `history.replaceState`, and sends the token in
          the `X-Share-Token` header on its one `/data` fetch. A URL
          fragment + custom header keeps the raw secret out of every
          reverse-proxy access log on the path.
        */}
        <Route path="/recordings/:id/play" component={RecordingPlayer} />
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
        <Route
          path="/admin/users"
          component={() => (
            <AuthGuard>
              <AdminGuard>
                <AdminUsers />
              </AdminGuard>
            </AuthGuard>
          )}
        />
        <Route
          path="/admin/audit"
          component={() => (
            <AuthGuard>
              <AdminGuard>
                <AdminAudit />
              </AdminGuard>
            </AuthGuard>
          )}
        />
        <Route
          path="/admin/system"
          component={() => (
            <AuthGuard>
              <AdminGuard><AdminSystem /></AdminGuard>
            </AuthGuard>
          )}
        />
      </Router>
      <ToastContainer />
    </I18nProvider>
  );
}
