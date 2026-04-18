// web/src/pages/Register.tsx
import { createSignal, onCleanup, Show } from 'solid-js';
import { useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { MIN_PASSWORD_LENGTH } from '../lib/protocol';
import { useI18n, renderTemplate } from '../i18n';
import LocaleSwitcher from '../components/LocaleSwitcher';

type Step = 'form' | 'otp' | 'pending';

const RESEND_COOLDOWN_SECS = 60;

export default function Register() {
  const { t } = useI18n();
  const navigate = useNavigate();

  const [step, setStep] = createSignal<Step>('form');
  const [name, setName] = createSignal('');
  const [email, setEmail] = createSignal('');
  const [password, setPassword] = createSignal('');
  const [code, setCode] = createSignal('');

  // Resend countdown state
  const [countdown, setCountdown] = createSignal(0);
  const [resending, setResending] = createSignal(false);
  const [resendFeedback, setResendFeedback] = createSignal('');
  let countdownTimer: ReturnType<typeof setInterval> | undefined;

  function startCountdown() {
    clearInterval(countdownTimer);
    setCountdown(RESEND_COOLDOWN_SECS);
    countdownTimer = setInterval(() => {
      setCountdown((prev) => {
        if (prev <= 1) {
          clearInterval(countdownTimer);
          return 0;
        }
        return prev - 1;
      });
    }, 1000);
  }

  onCleanup(() => clearInterval(countdownTimer));

  const handleRegister = async (e: Event) => {
    e.preventDefault();
    if (password().length < MIN_PASSWORD_LENGTH) {
      auth.setErrorKey('auth.error_password_too_short');
      return;
    }
    const ok = await auth.emailRegister(email(), password(), name());
    if (ok) {
      setStep('otp');
      startCountdown();
    }
  };

  const handleResend = async () => {
    if (resending() || countdown() > 0) return;
    setResending(true);
    setResendFeedback('');
    const ok = await auth.resendOtp(email(), password(), name());
    setResending(false);
    if (ok) {
      setResendFeedback(t('register.resend_sent'));
      setCode('');
      startCountdown();
    }
  };

  const handleVerify = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.emailVerifyOtp(email(), code());
    if (ok) {
      // Check if account is pending admin approval
      if (auth.currentUserSessionEnabled() === false && !auth.currentUserIsAdmin()) {
        setStep('pending');
      } else {
        navigate('/', { replace: true });
      }
    }
  };

  return (
    <div class="register-page">
      <div class="register-card">
        <h1>TELEPAIR</h1>

        {/* ── Step 1: Registration form ── */}
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

          {/*
            SMTP-fallback help panel. When the server answers 503 on
            `/api/auth/register` the UI previously showed only the
            terse `error_smtp_unavailable` line, which stranded
            self-hosted operators whose single-node install has no
            mail config. Surfacing the CLI bypass inline (with the
            operator's email/name pre-filled) gives them an
            actionable next step without forcing them to dig through
            `--help`. Fix for QA v0.1.9 C1/Q3.
          */}
          <Show when={auth.errorKey() === 'auth.error_smtp_unavailable'}>
            <div class="smtp-fallback" data-testid="smtp-fallback-help">
              <p class="smtp-fallback-title">{t('register.smtp_fallback_title')}</p>
              <p class="smtp-fallback-body">{t('register.smtp_fallback_body')}</p>
              <pre class="smtp-fallback-cli">
                <code>
                  {t('register.smtp_fallback_cli', {
                    email: email() || 'you@example.com',
                    name: name() || 'Your Name',
                  })}
                </code>
              </pre>
              <p class="smtp-fallback-tip">{t('register.smtp_fallback_cli_tip')}</p>
            </div>
          </Show>

          <p class="alt-link">
            {t('register.already_have_account')}{' '}
            <a href="/login">{t('register.login_link')}</a>
          </p>
        </Show>

        {/* ── Step 2: OTP verification ── */}
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

            <Show when={resendFeedback()}>
              <p class="resend-feedback">{resendFeedback()}</p>
            </Show>

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

          <div class="otp-actions">
            <button
              type="button"
              class="resend-btn"
              onClick={handleResend}
              disabled={resending() || countdown() > 0}
            >
              {resending()
                ? t('register.resend_sending')
                : countdown() > 0
                  ? renderTemplate(t('register.resend_countdown'), { seconds: String(countdown()) })
                  : t('register.resend')}
            </button>
            <button type="button" class="back-btn" onClick={() => setStep('form')}>
              {t('register.back')}
            </button>
          </div>
        </Show>

        {/* ── Step 3: Pending admin approval ── */}
        <Show when={step() === 'pending'}>
          <div class="pending-step">
            <div class="pending-icon">&#10003;</div>
            <h2>{t('register.pending_title')}</h2>
            <p class="pending-body">{t('register.pending_body')}</p>
            <button
              type="button"
              class="primary"
              onClick={() => navigate('/', { replace: true })}
            >
              {t('register.pending_go_dashboard')}
            </button>
          </div>
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
        .resend-feedback {
          color: var(--success, #22c55e);
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
        .otp-actions {
          display: flex;
          justify-content: space-between;
          align-items: center;
          margin-top: 12px;
        }
        .resend-btn {
          background: transparent;
          border: none;
          color: var(--accent);
          font-size: 13px;
          padding: 4px 8px;
          cursor: pointer;
        }
        .resend-btn:hover:not(:disabled) {
          text-decoration: underline;
          background: transparent;
        }
        .resend-btn:disabled {
          color: var(--text-secondary);
          cursor: default;
        }
        .back-btn {
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
        .pending-step {
          padding: 8px 0;
        }
        .pending-icon {
          width: 48px;
          height: 48px;
          border-radius: 50%;
          background: var(--success, #22c55e);
          color: #fff;
          font-size: 24px;
          line-height: 48px;
          margin: 0 auto 16px;
        }
        .pending-step h2 {
          font-size: 20px;
          font-weight: 600;
          margin-bottom: 8px;
        }
        .pending-body {
          color: var(--text-secondary);
          font-size: 14px;
          line-height: 1.5;
          margin-bottom: 20px;
        }
        .pending-step button {
          width: 100%;
          padding: 10px;
          font-size: 15px;
        }
        .smtp-fallback {
          margin-top: 16px;
          padding: 14px 16px;
          border: 1px solid var(--border);
          border-radius: 8px;
          background: var(--bg-tertiary, var(--bg-secondary));
          text-align: left;
        }
        .smtp-fallback-title {
          font-size: 13px;
          font-weight: 600;
          color: var(--text-primary);
          margin-bottom: 6px;
        }
        .smtp-fallback-body {
          font-size: 12px;
          color: var(--text-secondary);
          line-height: 1.5;
          margin-bottom: 10px;
        }
        .smtp-fallback-cli {
          display: block;
          font-family: var(--font-mono);
          font-size: 12px;
          padding: 8px 10px;
          background: var(--bg-primary);
          border: 1px solid var(--border);
          border-radius: 6px;
          overflow-x: auto;
          white-space: pre;
          margin: 0 0 8px;
          color: var(--text-primary);
        }
        .smtp-fallback-tip {
          font-size: 11px;
          color: var(--text-secondary);
          line-height: 1.5;
          margin: 0;
        }
      `}</style>
    </div>
  );
}
