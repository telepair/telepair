// web/src/pages/Login.tsx
import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { useI18n, renderTemplate } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';

type LoginMode = 'token' | 'email';

export default function Login() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [mode, setMode] = createSignal<LoginMode>('email');

  // Token mode
  const [tokenInput, setTokenInput] = createSignal('');
  const [showHelp, setShowHelp] = createSignal(false);

  // Email mode
  const [email, setEmail] = createSignal('');
  const [password, setPassword] = createSignal('');

  const handleTokenSubmit = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.validateToken(tokenInput());
    if (ok) navigate('/', { replace: true });
  };

  const handleEmailSubmit = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.emailLogin(email(), password());
    if (ok) navigate('/', { replace: true });
  };

  return (
    <div class="login-page">
      <div class="login-card">
        <h1>TELEPAIR</h1>
        <p class="subtitle">{t('login.subtitle')}</p>

        <div class="mode-tabs" role="tablist">
          <button
            role="tab"
            aria-selected={mode() === 'email'}
            class={`mode-tab ${mode() === 'email' ? 'active' : ''}`}
            onClick={() => { setMode('email'); auth.clearError(); }}
          >
            {t('login.mode_email')}
          </button>
          <button
            role="tab"
            aria-selected={mode() === 'token'}
            class={`mode-tab ${mode() === 'token' ? 'active' : ''}`}
            onClick={() => { setMode('token'); auth.clearError(); }}
          >
            {t('login.mode_token')}
          </button>
        </div>

        <Show when={mode() === 'email'}>
          <form onSubmit={handleEmailSubmit}>
            <label for="email">{t('login.email_label')}</label>
            <input
              id="email"
              type="email"
              placeholder={t('login.email_placeholder')}
              value={email()}
              onInput={(e) => setEmail(e.currentTarget.value)}
              autocomplete="email"
              autofocus
            />
            <label for="password">{t('login.password_label')}</label>
            <input
              id="password"
              type="password"
              placeholder={t('login.password_placeholder')}
              value={password()}
              onInput={(e) => setPassword(e.currentTarget.value)}
              autocomplete="current-password"
            />

            <Show when={auth.errorKey()}>
              {(key) => <p class="error-msg">{t(key())}</p>}
            </Show>

            <button
              type="submit"
              class="primary"
              disabled={auth.validating() || !email() || !password()}
            >
              {auth.validating() ? t('login.signing_in') : t('login.sign_in')}
            </button>
          </form>

          <p class="register-hint">
            {t('login.no_account')}{' '}
            <a href="/register">{t('login.register_link')}</a>
          </p>
        </Show>

        <Show when={mode() === 'token'}>
          <form onSubmit={handleTokenSubmit}>
            <label for="token">{t('login.token_label')}</label>
            <input
              id="token"
              type="password"
              placeholder={t('login.token_placeholder')}
              value={tokenInput()}
              onInput={(e) => setTokenInput(e.currentTarget.value)}
              autofocus
            />

            <Show when={auth.errorKey()}>
              {(key) => <p class="error-msg">{t(key())}</p>}
            </Show>

            <button type="submit" class="primary" disabled={auth.validating() || !tokenInput()}>
              {auth.validating() ? t('login.validating') : t('login.connect')}
            </button>
          </form>

          <button
            type="button"
            class="help-toggle"
            onClick={() => setShowHelp(!showHelp())}
            aria-expanded={showHelp()}
          >
            {showHelp() ? t('login.help_hide') : t('login.help_show')}
          </button>

          <Show when={showHelp()}>
            <div class="help-panel">
              <p>{renderTemplate(
                t('login.help_first_run'),
                { path: <code>~/.telepair/admin_token</code> },
              )}</p>
              <p>{renderTemplate(
                t('login.help_lost'),
                { cmd: <code>telepair admin show-token</code> },
              )}</p>
              <p>{t('login.help_joining')}</p>
            </div>
          </Show>
        </Show>

        <LocaleSwitcher variant="card" />
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
          margin-bottom: 20px;
          font-size: 14px;
        }
        .mode-tabs {
          display: flex;
          gap: 4px;
          background: var(--bg-tertiary);
          border-radius: 8px;
          padding: 4px;
          margin-bottom: 20px;
        }
        .mode-tab {
          flex: 1;
          background: transparent;
          border: none;
          border-radius: 6px;
          padding: 7px 12px;
          font-size: 13px;
          font-weight: 500;
          color: var(--text-secondary);
          cursor: pointer;
          transition: background 0.15s, color 0.15s;
        }
        .mode-tab:hover {
          color: var(--text-primary);
        }
        .mode-tab.active {
          background: var(--bg-secondary);
          color: var(--text-primary);
          box-shadow: 0 1px 3px rgba(0,0,0,0.15);
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
          margin-bottom: 14px;
        }
        .login-card button[type="submit"] {
          width: 100%;
          padding: 10px;
          font-size: 15px;
          margin-top: 2px;
        }
        .error-msg {
          color: var(--error);
          font-size: 13px;
          margin-bottom: 12px;
        }
        .register-hint {
          margin-top: 16px;
          font-size: 13px;
          color: var(--text-secondary);
        }
        .register-hint a {
          color: var(--accent);
          text-decoration: none;
          font-weight: 500;
        }
        .register-hint a:hover {
          text-decoration: underline;
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
