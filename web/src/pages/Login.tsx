// web/src/pages/Login.tsx
import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';

export default function Login() {
  const navigate = useNavigate();
  const [input, setInput] = createSignal('');
  const [showHelp, setShowHelp] = createSignal(false);

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

        <button
          type="button"
          class="help-toggle"
          onClick={() => setShowHelp(!showHelp())}
          aria-expanded={showHelp()}
        >
          {showHelp() ? 'Hide help' : "Don't have a token?"}
        </button>

        <Show when={showHelp()}>
          <div class="help-panel">
            <p>
              <strong>First run?</strong> telepair prints the admin token to the
              server console on startup and saves it to{' '}
              <code>~/.telepair/admin_token</code>.
            </p>
            <p>
              <strong>Lost it?</strong> Run{' '}
              <code>telepair admin show-token</code> on the server to print it
              again.
            </p>
            <p>
              <strong>Joining a session?</strong> Ask the session owner to send
              you an invite link — you'll still need a token of your own.
            </p>
          </div>
        </Show>
      </div>

      <style>{`
        .login-page {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
          padding: 16px;
        }
        .login-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 12px;
          padding: 40px;
          width: 380px;
          max-width: 100%;
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
        .login-card button[type="submit"] {
          width: 100%;
          padding: 10px;
          font-size: 15px;
        }
        .error-msg {
          color: var(--error);
          font-size: 13px;
          margin-bottom: 12px;
        }
        .help-toggle {
          margin-top: 16px;
          background: transparent;
          border: none;
          color: var(--text-secondary);
          font-size: 12px;
          padding: 4px 8px;
          cursor: pointer;
        }
        .help-toggle:hover {
          color: var(--accent);
          background: transparent;
        }
        .help-panel {
          margin-top: 12px;
          padding: 16px;
          background: var(--bg-tertiary);
          border: 1px solid var(--border);
          border-radius: 8px;
          text-align: left;
          font-size: 12.5px;
          line-height: 1.6;
          color: var(--text-secondary);
        }
        .help-panel p {
          margin-bottom: 10px;
        }
        .help-panel p:last-child {
          margin-bottom: 0;
        }
        .help-panel strong {
          color: var(--text-primary);
          font-weight: 600;
        }
        .help-panel code {
          font-family: var(--font-mono);
          font-size: 11.5px;
          padding: 1px 5px;
          background: var(--bg-primary);
          border: 1px solid var(--border);
          border-radius: 3px;
          color: var(--accent);
        }
      `}</style>
    </div>
  );
}
