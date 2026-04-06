import { onMount, createSignal, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { api, ApiError } from '../lib/api';

export default function Join() {
  const params = useParams<{ token: string }>();
  const navigate = useNavigate();
  const [error, setError] = createSignal('');
  const [redeeming, setRedeeming] = createSignal(true);

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
      }
      navigate(`/session/${result.session_id}`, { replace: true });
    } catch (e) {
      setRedeeming(false);
      if (e instanceof ApiError) {
        if (e.status === 400) {
          setError('Invalid or expired invite link');
        } else if (e.status === 410) {
          setError('This session has been closed');
        } else {
          setError(e.message);
        }
      } else {
        setError('Failed to join session');
      }
    }
  });

  return (
    <div class="join-page">
      <div class="join-card">
        <h1>telepair</h1>
        <Show when={redeeming()} fallback={
          <div>
            <p class="error-msg">{error()}</p>
            <button class="primary" onClick={() => navigate('/')}>Go to Dashboard</button>
          </div>
        }>
          <p class="muted">Joining session...</p>
        </Show>
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
