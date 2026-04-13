// web/src/pages/Register.tsx
import { createSignal, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { useI18n, renderTemplate } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';

type Step = 'form' | 'otp';

export default function Register() {
  const { t } = useI18n();
  const navigate = useNavigate();

  const [step, setStep] = createSignal<Step>('form');
  const [name, setName] = createSignal('');
  const [email, setEmail] = createSignal('');
  const [password, setPassword] = createSignal('');
  const [code, setCode] = createSignal('');

  const handleRegister = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.emailRegister(name(), email(), password());
    if (ok) setStep('otp');
  };

  const handleVerify = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.emailVerifyOtp(email(), code());
    if (ok) navigate('/', { replace: true });
  };

  return (
    <div class="register-page">
      <div class="register-card">
        <h1>telepair</h1>

        <Show when={step() === 'form'}>
          <p class="subtitle">{t('register.subtitle')}</p>

          <form onSubmit={handleRegister}>
            <label for="name">{t('register.name_label')}</label>
            <input
              id="name"
              type="text"
              placeholder={t('register.name_placeholder')}
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
              autocomplete="name"
              autofocus
            />

            <label for="email">{t('register.email_label')}</label>
            <input
              id="email"
              type="email"
              placeholder={t('register.email_placeholder')}
              value={email()}
              onInput={(e) => setEmail(e.currentTarget.value)}
              autocomplete="email"
            />

            <label for="password">{t('register.password_label')}</label>
            <input
              id="password"
              type="password"
              placeholder={t('register.password_placeholder')}
              value={password()}
              onInput={(e) => setPassword(e.currentTarget.value)}
              autocomplete="new-password"
            />

            <Show when={auth.errorKey()}>
              {(key) => <p class="error-msg">{t(key())}</p>}
            </Show>

            <button
              type="submit"
              class="primary"
              disabled={auth.validating() || !name() || !email() || !password()}
            >
              {auth.validating() ? t('register.submitting') : t('register.next')}
            </button>
          </form>

          <p class="alt-link">
            {t('register.already_have_account')}{' '}
            <a href="/login">{t('register.login_link')}</a>
          </p>
        </Show>

        <Show when={step() === 'otp'}>
          <p class="subtitle">
            {renderTemplate(t('register.otp_subtitle'), { email: <strong>{email()}</strong> })}
          </p>

          <form onSubmit={handleVerify}>
            <label for="code">{t('register.otp_label')}</label>
            <input
              id="code"
              type="text"
              inputMode="numeric"
              pattern="[0-9]{6}"
              maxLength={6}
              placeholder={t('register.otp_placeholder')}
              value={code()}
              onInput={(e) => setCode(e.currentTarget.value.replace(/\D/g, '').slice(0, 6))}
              autocomplete="one-time-code"
              autofocus
            />

            <Show when={auth.errorKey()}>
              {(key) => <p class="error-msg">{t(key())}</p>}
            </Show>

            <button
              type="submit"
              class="primary"
              disabled={auth.validating() || code().length !== 6}
            >
              {auth.validating() ? t('register.verifying') : t('register.verify')}
            </button>
          </form>

          <button type="button" class="back-btn" onClick={() => setStep('form')}>
            {t('register.back')}
          </button>
        </Show>

        <LocaleSwitcher variant="card" />
      </div>

      <style>{`
        .register-page {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
          padding: 16px;
        }
        .register-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 12px;
          padding: 40px;
          width: 380px;
          max-width: 100%;
          text-align: center;
        }
        .register-card h1 {
          font-size: 28px;
          font-weight: 700;
          margin-bottom: 4px;
        }
        .subtitle {
          color: var(--text-secondary);
          margin-bottom: 20px;
          font-size: 14px;
        }
        .register-card form {
          text-align: left;
        }
        .register-card label {
          display: block;
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          margin-bottom: 6px;
        }
        .register-card input {
          margin-bottom: 14px;
        }
        .register-card button[type="submit"] {
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
        .alt-link {
          margin-top: 16px;
          font-size: 13px;
          color: var(--text-secondary);
        }
        .alt-link a {
          color: var(--accent);
          text-decoration: none;
          font-weight: 500;
        }
        .alt-link a:hover {
          text-decoration: underline;
        }
        .back-btn {
          margin-top: 12px;
          background: transparent;
          border: none;
          color: var(--text-secondary);
          font-size: 13px;
          padding: 4px 8px;
          cursor: pointer;
        }
        .back-btn:hover {
          color: var(--accent);
          background: transparent;
        }
      `}</style>
    </div>
  );
}
