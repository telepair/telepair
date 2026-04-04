// web/src/pages/Login.tsx
import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';

export default function Login() {
  const navigate = useNavigate();
  const [input, setInput] = createSignal('');

  const handleSubmit = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.validateToken(input());
    if (ok) {
      const pendingInvite = sessionStorage.getItem('pending_invite');
      if (pendingInvite) {
        sessionStorage.removeItem('pending_invite');
        navigate(`/join/${pendingInvite}`, { replace: true });
      } else {
        navigate('/', { replace: true });
      }
    }
  };

  return (
    <div class="login-page">
      <div class="login-card">
        <h1>telepair</h1>
        <p class="subtitle">Web terminal collaboration</p>

        <form onSubmit={handleSubmit}>
          <label for="token">API Token</label>
          <input
            id="token"
            type="password"
            placeholder="Paste your token here"
            value={input()}
            onInput={(e) => setInput(e.currentTarget.value)}
            autofocus
          />

          <Show when={auth.error()}>
            <p class="error-msg">{auth.error()}</p>
          </Show>

          <button type="submit" class="primary" disabled={auth.validating() || !input()}>
            {auth.validating() ? 'Validating...' : 'Connect'}
          </button>
        </form>
      </div>

      <style>{`
        .login-page {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
        }
        .login-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 12px;
          padding: 40px;
          width: 380px;
          text-align: center;
        }
        .login-card h1 {
          font-size: 28px;
          font-weight: 700;
          margin-bottom: 4px;
        }
        .subtitle {
          color: var(--text-secondary);
          margin-bottom: 24px;
          font-size: 14px;
        }
        .login-card form {
          text-align: left;
        }
        .login-card label {
          display: block;
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          margin-bottom: 6px;
        }
        .login-card input {
          margin-bottom: 16px;
        }
        .login-card button {
          width: 100%;
          padding: 10px;
          font-size: 15px;
        }
        .error-msg {
          color: var(--error);
          font-size: 13px;
          margin-bottom: 12px;
        }
      `}</style>
    </div>
  );
}
