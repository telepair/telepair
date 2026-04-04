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
    if (!auth.isAuthenticated()) {
      sessionStorage.setItem('pending_invite', params.token);
      navigate('/login', { replace: true });
      return;
    }

    try {
      const result = await api.redeemInvite(params.token);
      navigate(`/session/${result.session_id}`, { replace: true });
    } catch (e) {
      setRedeeming(false);
      if (e instanceof ApiError) {
        setError(e.status === 400 ? 'Invalid or expired invite link' : e.message);
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
