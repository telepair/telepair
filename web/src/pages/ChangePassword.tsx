// web/src/pages/ChangePassword.tsx
import { createSignal, Show } from 'solid-js';
import { useI18n } from '../i18n';
import { api, ApiError, errorMessage } from '../lib/api';
import { MIN_PASSWORD_LENGTH } from '../lib/protocol';
import { auth } from '../stores/auth';
import LocaleSwitcher from '../components/LocaleSwitcher';

export default function ChangePassword() {
  const { t } = useI18n();

  const [currentPw, setCurrentPw] = createSignal('');
  const [newPw, setNewPw] = createSignal('');
  const [confirmPw, setConfirmPw] = createSignal('');
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal('');
  const [success, setSuccess] = createSignal(false);

  const handleSubmit = async (e: Event) => {
    e.preventDefault();
    setError('');

    if (newPw().length < MIN_PASSWORD_LENGTH) {
      setError(t('change_password.error_too_short'));
      return;
    }
    if (newPw() !== confirmPw()) {
      setError(t('change_password.error_mismatch'));
      return;
    }

    setSubmitting(true);
    try {
      const { token } = await api.changePassword(currentPw(), newPw());
      // The server rotated the bearer token — update the auth store
      // so subsequent requests use the new credential.
      auth.setToken(token, { persist: true });
      setSuccess(true);
      setCurrentPw('');
      setNewPw('');
      setConfirmPw('');
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        // 401 from change-password means wrong current password
        setError(e.message);
      } else {
        setError(errorMessage(e));
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div class="cp-page">
      <div class="cp-card">
        <h1>TELEPAIR</h1>
        <p class="subtitle">{t('change_password.title')}</p>

        <Show when={success()}>
          <div class="success-msg">{t('change_password.success')}</div>
        </Show>

        <form onSubmit={handleSubmit}>
          <label for="current-pw">{t('change_password.current_label')}</label>
          <input
            id="current-pw"
            type="password"
            placeholder={t('change_password.current_placeholder')}
            value={currentPw()}
            onInput={(e) => { setCurrentPw(e.currentTarget.value); setSuccess(false); }}
            autocomplete="current-password"
            autofocus
          />

          <label for="new-pw">{t('change_password.new_label')}</label>
          <input
            id="new-pw"
            type="password"
            placeholder={t('change_password.new_placeholder')}
            value={newPw()}
            onInput={(e) => setNewPw(e.currentTarget.value)}
            autocomplete="new-password"
          />

          <label for="confirm-pw">{t('change_password.confirm_label')}</label>
          <input
            id="confirm-pw"
            type="password"
            placeholder={t('change_password.confirm_placeholder')}
            value={confirmPw()}
            onInput={(e) => setConfirmPw(e.currentTarget.value)}
            autocomplete="new-password"
          />

          <Show when={error()}>
            <p class="error-msg">{error()}</p>
          </Show>

          <button
            type="submit"
            class="primary"
            disabled={submitting() || !currentPw() || !newPw() || !confirmPw()}
          >
            {submitting() ? t('change_password.submitting') : t('change_password.submit')}
          </button>
        </form>

        <a href="/" class="back-link">{t('change_password.back')}</a>

        <LocaleSwitcher variant="card" />
      </div>

      <style>{`
        .cp-page {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
          padding: 16px;
        }
        .cp-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 12px;
          padding: 40px;
          width: 380px;
          max-width: 100%;
          text-align: center;
        }
        .cp-card h1 {
          font-size: 28px;
          font-weight: 700;
          margin-bottom: 4px;
        }
        .subtitle {
          color: var(--text-secondary);
          margin-bottom: 20px;
          font-size: 14px;
        }
        .cp-card form {
          text-align: left;
        }
        .cp-card label {
          display: block;
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          margin-bottom: 6px;
        }
        .cp-card input {
          margin-bottom: 14px;
        }
        .cp-card button[type="submit"] {
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
        .success-msg {
          color: var(--success, #22c55e);
          font-size: 13px;
          margin-bottom: 16px;
          padding: 8px 12px;
          background: rgba(63, 185, 80, 0.12);
          border-radius: 6px;
        }
        .back-link {
          display: inline-block;
          margin-top: 16px;
          color: var(--text-secondary);
          font-size: 13px;
          text-decoration: none;
        }
        .back-link:hover {
          color: var(--accent);
        }
      `}</style>
    </div>
  );
}
