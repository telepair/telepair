import { onMount, createSignal, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { api, ApiError } from '../lib/api';
import { useI18n, type TranslationKey } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';

export default function Join() {
  const { t } = useI18n();
  const params = useParams<{ token: string }>();
  const navigate = useNavigate();
  // Mirrors the `auth.errorKey` / `Session.errorKey` pattern: store the
  // i18n key for our own messages so a locale switch re-renders them
  // live, and store the raw server message in a parallel signal for
  // pass-through cases that have no translation.
  const [errorKey, setErrorKey] = createSignal<TranslationKey | null>(null);
  const [errorText, setErrorText] = createSignal('');
  const [redeeming, setRedeeming] = createSignal(true);

  const errorMessage = (): string => {
    const k = errorKey();
    if (k) return t(k);
    return errorText();
  };

  onMount(async () => {
    // The redeem endpoint accepts anonymous callers: if the visitor
    // has no token, the backend mints a fresh guest account and
    // returns its token in the response. That removes the old
    // "please paste a token first" wall that blocked every new
    // collaborator. If we DO already have a token (the inviter
    // testing their own link, for instance), the backend reuses
    // that identity and returns `token: null`.
    try {
      const result = await api.redeemInvite(params.token);
      if (result.token) {
        auth.setToken(result.token);
        // A token swap invalidates the cached whoami (see
        // `setToken` for the rationale). Prime the new identity here
        // so Session.tsx, AdminGuard, and the dashboard owner gate
        // all see the guest's `is_guest=true` flag on first render —
        // without this, the session page briefly runs with the
        // previous user's identity (if any) and the back-button
        // dispatch picks the wrong branch.
        await auth.loadIdentity();
      }
      navigate(`/session/${result.session_id}`, { replace: true });
    } catch (e) {
      setRedeeming(false);
      if (e instanceof ApiError) {
        if (e.status === 400) {
          setErrorKey('join.error_invalid');
        } else if (e.status === 410) {
          setErrorKey('join.error_closed');
        } else {
          // Server-provided error message — already in English by design.
          setErrorText(e.message);
        }
      } else {
        setErrorKey('join.error_failed');
      }
    }
  });

  return (
    <div class="join-page">
      <div class="join-card">
        <h1>telepair</h1>
        <Show when={redeeming()} fallback={
          <div>
            <p class="error-msg">{errorMessage()}</p>
            <button class="primary" onClick={() => navigate('/')}>{t('join.go_dashboard')}</button>
          </div>
        }>
          <p class="muted">{t('join.joining')}</p>
        </Show>
        <LocaleSwitcher variant="card" />
      </div>
      <style>{`
        .join-page { display: flex; align-items: center; justify-content: center; min-height: 100vh; }
        .join-card { background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; padding: 40px; width: 380px; text-align: center; }
        .join-card h1 { font-size: 28px; font-weight: 700; margin-bottom: 16px; }
        .muted { color: var(--text-secondary); }
        .error-msg { color: var(--error); margin-bottom: 16px; }
      `}</style>
    </div>
  );
}
