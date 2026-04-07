// web/src/pages/Login.tsx
import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { useI18n, renderTemplate } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';

export default function Login() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [input, setInput] = createSignal('');
  const [showHelp, setShowHelp] = createSignal(false);

  const handleSubmit = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.validateToken(input());
    if (ok) {
      navigate('/', { replace: true });
    }
  };

  return (
    <div class="login-page">
      <div class="login-card">
        <h1>telepair</h1>
        <p class="subtitle">{t('login.subtitle')}</p>

        <form onSubmit={handleSubmit}>
          <label for="token">{t('login.token_label')}</label>
          <input
            id="token"
            type="password"
            placeholder={t('login.token_placeholder')}
            value={input()}
            onInput={(e) => setInput(e.currentTarget.value)}
            autofocus
          />

          <Show when={auth.errorKey()}>
            {(key) => <p class="error-msg">{t(key())}</p>}
          </Show>

          <button type="submit" class="primary" disabled={auth.validating() || !input()}>
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
            {/* The help text mixes translated copy with literal <code>
                tags for the on-disk path and CLI command. Splitting on
                `{{ path }}` / `{{ cmd }}` lets the translation control
                the surrounding sentence while keeping the technical
                tokens untranslated and visually distinct. */}
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
