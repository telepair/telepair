use std::sync::{Arc, LazyLock};

use argon2::password_hash::{SaltString, rand_core::OsRng};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{Duration, Utc};

use telepair_core::audit::{AuditEvent, AuditEventType, AuditSink};
use telepair_core::error::{Error, Result};
use telepair_core::session::{LoginFailureOutcome, PendingVerifyResult, User};
use telepair_core::storage::{AccountFilter, SqliteStorage, Storage};
use uuid::Uuid;

const OTP_TTL_MINUTES: i64 = 15;
const OTP_RATE_LIMIT_SECS: i64 = 60;
/// Window the password-login lockout holds the row for after the
/// 5-strike threshold is hit. Mirrors the OTP TTL — short enough that
/// a real user can shake it off after a coffee break, long enough that
/// a credential-stuffing run can't sustain meaningful throughput.
const LOGIN_LOCKOUT_MINUTES: i64 = 15;
const MIN_PASSWORD_LENGTH: usize = 8;

fn validate_password(password: &str) -> Result<()> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(Error::InvalidInput(format!(
            "Password must be at least {MIN_PASSWORD_LENGTH} characters."
        )));
    }
    Ok(())
}

/// Single user-facing string returned for every failure of
/// `verify_otp` and `login`. The whole point of the unified shape is
/// that an unauthenticated caller cannot distinguish "unknown email"
/// from "wrong password" from "wrong code" from "expired pending row"
/// — the audit log carries the precise reason for the operator, the
/// API does not.
const GENERIC_AUTH_ERROR: &str = "invalid email or code";

/// Throw-away Argon2 hash generated once per process from a constant
/// placeholder password. Used by [`AuthService::burn_dummy_verify`] to
/// equalise the wall-clock cost of the early-return branches in
/// `login` (unknown email, user row without a password hash) with the
/// wrong-password branch — closing the email-enumeration timing side
/// channel the `GENERIC_AUTH_ERROR` shape would otherwise leak.
///
/// Regenerated on every process start so it can never collide with a
/// real user's stored hash. The actual password hashed is irrelevant;
/// we only care that `verify_password` against this hash costs the
/// same as a real verify (both go through `Argon2::default()`).
///
/// `LazyLock` defers the ~50 ms hashing cost until the first login
/// attempt rather than paying it on every process boot.
static DUMMY_LOGIN_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("timing-shield-placeholder-do-not-use")
        .expect("argon2 hash of a constant string cannot fail")
});

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
}

pub struct AuthService {
    storage: Arc<SqliteStorage>,
    /// Pre-built transport for connection reuse. `None` when SMTP is not configured.
    mailer: Option<Arc<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>>>,
    /// From address for outgoing emails.
    smtp_from: Option<String>,
    /// Audit sink for failed-login telemetry. Login is the only path
    /// in this service that emits audit events today, but giving the
    /// service the sink rather than threading it through every call
    /// site keeps the route handler in `gateway/http.rs` simple
    /// (`state.auth_service.login(&email, &password).await?`).
    audit: Arc<AuditSink>,
}

impl AuthService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        smtp: Option<Arc<SmtpConfig>>,
        audit: Arc<AuditSink>,
    ) -> Self {
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, Tokio1Executor};

        let (mailer, smtp_from) = match smtp {
            None => (None, None),
            Some(cfg) => {
                let creds = Credentials::new(cfg.username.clone(), cfg.password.clone());
                // `relay()` only validates the host string; it does not open a
                // connection. A bad host is a startup misconfiguration — fail loudly.
                let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
                    .expect("invalid SMTP host in configuration")
                    .port(cfg.port)
                    .credentials(creds)
                    .build();
                (Some(Arc::new(transport)), Some(cfg.from.clone()))
            }
        };
        Self {
            storage,
            mailer,
            smtp_from,
            audit,
        }
    }

    /// Register a new user. Hashes password, writes (or overwrites in
    /// place) a row in `pending_registrations`, then sends an OTP to
    /// the address. The pending row carries no authority of its own —
    /// no `users` entry, no token — so even if the same email is
    /// re-registered moments later by an attacker, no live account
    /// can be hijacked.
    ///
    /// Returns `ServiceUnavailable` if SMTP is not configured. Every
    /// other error mode (already verified, already pending) is
    /// collapsed into `Ok(())` so an unauthenticated caller cannot
    /// distinguish "we sent you a code" from "this address already
    /// has an account" — the audit log captures the precise reason.
    pub async fn register(&self, email: &str, password: &str, display_name: &str) -> Result<()> {
        let email = email.to_lowercase();
        let email = email.as_str();
        let mailer = self.mailer.as_ref().ok_or_else(|| {
            Error::ServiceUnavailable(
                "This instance has not configured email sending. Contact the administrator.".into(),
            )
        })?;
        let smtp_from = self.smtp_from.as_deref().ok_or_else(|| {
            Error::Internal("SMTP from address missing despite mailer being configured".into())
        })?;

        // Enumeration safety: if the address already maps to a real
        // user row, do not write a pending row and do not send a code.
        // Pretending the request succeeded leaks no information; the
        // existing user has been told (out of band, on a previous
        // signup) that they have an account. Audit the silent reject
        // so an operator can still see the attempt.
        // Both lookups are read-only against different tables and have
        // no data dependency — run them concurrently to save a round-trip.
        let (user_opt, last_pending) = tokio::join!(
            self.storage.get_user_by_email(email),
            self.storage.latest_pending_registration_at(email),
        );
        if user_opt?.is_some() {
            self.audit_register_silently_rejected(email, "already_registered")
                .await;
            return Ok(());
        }

        // Rate limit: if a pending row was refreshed within the OTP
        // rate-limit window, silently accept — same as already_registered
        // — so an attacker cannot distinguish "pending signup" from
        // "unknown email" via a 429 vs 2xx difference.
        //
        // This gate is per-email only. The complementary per-source-IP
        // gate lives one layer up in the HTTP handler
        // (`telepair_gateway::rate_limit::RegisterRateLimiter`) and
        // catches the "one attacker, many emails" shape that would
        // otherwise walk straight past this check.
        if let Some(last) = last_pending?
            && Utc::now() - last < Duration::seconds(OTP_RATE_LIMIT_SECS)
        {
            self.audit_register_silently_rejected(email, "rate_limited")
                .await;
            return Ok(());
        }

        validate_password(password)?;

        let password = password.to_owned();
        let hash = tokio::task::spawn_blocking(move || hash_password(&password))
            .await
            .map_err(|_| Error::Internal("hash task panicked".into()))??;

        let code = generate_otp();
        let expires = Utc::now() + Duration::minutes(OTP_TTL_MINUTES);
        self.storage
            .upsert_pending_registration(email, display_name, &hash, &code, expires)
            .await?;

        if let Err(send_err) = send_otp_email(mailer, smtp_from, email, &code).await {
            // SMTP failed: drop the pending row so the next register
            // attempt is not held off by the 60-second rate limit on
            // a code that was never delivered. The compare-and-delete
            // (`email + otp_code`) ensures a concurrent registration
            // that already overwrote the row with a new OTP is not
            // affected by this rollback.
            if let Err(rollback_err) = self.storage.delete_pending_registration(email, &code).await
            {
                tracing::warn!(
                    %email,
                    "failed to roll back pending registration after SMTP failure: {rollback_err}",
                );
            }
            return Err(send_err);
        }
        Ok(())
    }

    /// Verify an OTP code for the given email and complete the
    /// registration. Returns a bearer token on success. Every failure
    /// mode is collapsed into a single `Error::Auth(GENERIC_AUTH_ERROR)`
    /// so the API cannot be used to enumerate which addresses have
    /// pending rows. The detailed reason is still captured in the
    /// audit log so an operator can distinguish stuffing attempts
    /// from genuine typos.
    pub async fn verify_otp(&self, email: &str, code: &str) -> Result<String> {
        let email = email.to_lowercase();
        let email = email.as_str();
        let outcome = match self.storage.verify_pending_registration(email, code).await {
            Ok(r) => r,
            Err(Error::Conflict(msg)) => {
                tracing::warn!(%email, %msg, "display name collision during OTP verify");
                self.audit_verify_failed(email, "display_name_conflict", None)
                    .await;
                return Err(Error::Auth(GENERIC_AUTH_ERROR.into()));
            }
            Err(e) => return Err(e),
        };
        match outcome {
            PendingVerifyResult::Success { user, raw_token } => {
                self.audit_register_completed(user.id, &user.name, email)
                    .await;
                Ok(raw_token)
            }
            PendingVerifyResult::Failure { remaining } => {
                self.audit_verify_failed(email, "bad_code", Some(remaining))
                    .await;
                Err(Error::Auth(GENERIC_AUTH_ERROR.into()))
            }
            PendingVerifyResult::Locked => {
                self.audit_verify_failed(email, "locked", None).await;
                Err(Error::Auth(GENERIC_AUTH_ERROR.into()))
            }
            PendingVerifyResult::Expired => {
                self.audit_verify_failed(email, "expired_or_unknown", None)
                    .await;
                Err(Error::Auth(GENERIC_AUTH_ERROR.into()))
            }
        }
    }

    /// Authenticate with email + password. Returns a fresh bearer token.
    ///
    /// Throttle contract (Fix #3 — credential-stuffing defence):
    ///
    /// 1. **Unknown email**: returns `Auth(GENERIC_AUTH_ERROR)` — the
    ///    *same* error shape every other failure path uses, so the
    ///    response cannot be used to enumerate registered addresses.
    ///    The attempt is audited with `actor_id = None` so an operator
    ///    can still correlate hits across the same address.
    /// 2. **Currently locked**: returns `Auth(GENERIC_AUTH_ERROR)`
    ///    *before* hashing the candidate password — the timing-side
    ///    channel a hash check would open up is the whole reason the
    ///    lockout exists. The lockout is only visible to the audit
    ///    trail, not the user; the only signal a real user gets is
    ///    that retrying still fails until the window passes.
    /// 3. **Bad password**: increments the row's `login_failed_count`
    ///    via [`Storage::record_login_failure`]. The fifth strike
    ///    flips the row to `Locked` for `LOGIN_LOCKOUT_MINUTES` and
    ///    every entry into this branch records an `auth.login_failed`
    ///    audit row carrying the post-bump remaining count and the
    ///    lockout timestamp (when applicable).
    /// 4. **Good password**: clears the counter and lockout, then
    ///    issues a fresh bearer token. A single correct attempt wipes
    ///    out prior bad ones — the throttle is not a "failures over
    ///    all time" tally.
    ///
    /// Note that the `session_enabled` check does NOT happen here:
    /// login still mints a token for an admin-disabled account so the
    /// user can read history / change their password / etc. Every
    /// session-mutating surface enforces the bit on its own:
    ///
    /// - `POST /api/sessions`
    /// - `POST /api/sessions/{id}/invites` (mint) and
    ///   `DELETE /api/sessions/{id}/invites/{token}` (revoke)
    /// - `POST /api/invite/redeem` (for authenticated callers; the
    ///   anonymous guest path is unaffected since fresh guests are
    ///   minted with `session_enabled = TRUE`)
    /// - `PUT /api/sessions/{id}/participants/{user_id}/role`
    /// - `GET /ws/session/{id}` (WS attach)
    ///
    /// Conflating the gate with login would block a legitimate
    /// password reset flow.
    pub async fn login(&self, email: &str, password: &str) -> Result<String> {
        let email = email.to_lowercase();
        let email = email.as_str();
        let user = match self.storage.get_user_by_email(email).await? {
            Some(u) => u,
            None => {
                // Timing shield (Fix #P0-1 — email enumeration):
                // Burn one Argon2 verify against a throw-away hash so
                // the unknown-email branch takes the same wall-clock
                // time as the wrong-password branch below. Without
                // this, an unauthenticated attacker can enumerate
                // registered addresses by measuring the ~50 ms delta
                // an Argon2 verify introduces even though both paths
                // return the same `GENERIC_AUTH_ERROR` string.
                Self::burn_dummy_verify(password).await;
                self.audit_login_failed(None, email, "unknown_email", None, None)
                    .await;
                return Err(Error::Auth(GENERIC_AUTH_ERROR.into()));
            }
        };

        if let Some(until) = self.storage.check_login_lockout(user.id).await? {
            // Locked branch intentionally skips the Argon2 burn: a
            // locked row has already produced five prior unlocked
            // attempts that *did* hash, so the row's existence is not
            // newly leaked here, and paying the Argon2 cost on every
            // retry inside a lockout window would be a CPU-DoS gift
            // to a credential-stuffing run. The audit trail below
            // still captures the lockout hit.
            self.audit_login_failed(
                Some((user.id, &user.name)),
                email,
                "locked",
                None,
                Some(until),
            )
            .await;
            return Err(Error::Auth(GENERIC_AUTH_ERROR.into()));
        }

        let hash = match self.storage.get_password_hash(user.id).await? {
            Some(h) => h,
            None => {
                // A row with no password hash is either an admin/CLI
                // account (never used password login) or a data
                // integrity bug. Treat it as a bad-password attempt
                // for throttle accounting so a hammering attacker
                // still hits the lockout.
                //
                // Timing shield (Fix #P0-1 — email enumeration):
                // Burn one Argon2 verify so the shape of this branch
                // is wall-clock indistinguishable from the
                // wrong-password branch below. Without it, an
                // attacker observing the latency delta could single
                // out admin/CLI accounts (which have no password hash)
                // from regular users.
                Self::burn_dummy_verify(password).await;
                self.record_bad_password(&user.id, &user.name, email).await;
                return Err(Error::Auth(GENERIC_AUTH_ERROR.into()));
            }
        };

        let password_owned = password.to_owned();
        let verify =
            tokio::task::spawn_blocking(move || verify_password(&password_owned, &hash)).await;
        let verify = match verify {
            Ok(v) => v,
            Err(_) => return Err(Error::Internal("verify task panicked".into())),
        };
        if verify.is_err() {
            self.record_bad_password(&user.id, &user.name, email).await;
            return Err(Error::Auth(GENERIC_AUTH_ERROR.into()));
        }

        self.storage.clear_login_failures(user.id).await?;
        self.storage.refresh_user_token(user.id).await
    }

    /// Drive the throttle + audit on a confirmed bad-password attempt.
    /// Pulled out so the verified-but-no-hash path and the verify
    /// failure path don't drift.
    async fn record_bad_password(&self, user_id: &uuid::Uuid, user_name: &str, email: &str) {
        let outcome = self
            .storage
            .record_login_failure(*user_id, Duration::minutes(LOGIN_LOCKOUT_MINUTES))
            .await;
        let (remaining, until) = match outcome {
            Ok(LoginFailureOutcome::Recorded { remaining }) => (Some(remaining), None),
            Ok(LoginFailureOutcome::Locked { until }) => (Some(0), Some(until)),
            Err(e) => {
                tracing::error!(%e, %user_id, "failed to record login failure");
                (None, None)
            }
        };
        self.audit_login_failed(
            Some((*user_id, user_name)),
            email,
            "bad_password",
            remaining,
            until,
        )
        .await;
    }

    /// Run one Argon2 verify against a throw-away hash so an early-
    /// return branch in `login` takes the same wall-clock time as the
    /// wrong-password branch. The return value is ignored — the verify
    /// against `DUMMY_LOGIN_HASH` is expected to fail — we only care
    /// about the CPU cost.
    ///
    /// See [`DUMMY_LOGIN_HASH`] for the rationale. Kept as a private
    /// associated function so the call sites read as
    /// `Self::burn_dummy_verify(password)` and the intent is obvious.
    async fn burn_dummy_verify(password: &str) {
        let password_owned = password.to_owned();
        let _ = tokio::task::spawn_blocking(move || {
            verify_password(&password_owned, &DUMMY_LOGIN_HASH)
        })
        .await;
    }

    /// Build and emit a single `auth.login_failed` audit row. The
    /// detail JSON shape is described on
    /// [`AuditEventType::AuthLoginFailed`].
    async fn audit_login_failed(
        &self,
        actor: Option<(uuid::Uuid, &str)>,
        email: &str,
        reason: &str,
        remaining: Option<u32>,
        locked_until: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let mut detail = serde_json::json!({
            "email": email,
            "reason": reason,
        });
        if let Some(r) = remaining {
            detail["remaining"] = serde_json::json!(r);
        }
        if let Some(t) = locked_until {
            detail["locked_until"] = serde_json::json!(t.to_rfc3339());
        }
        let mut event = AuditEvent::new(AuditEventType::AuthLoginFailed).with_detail(detail);
        if let Some((id, name)) = actor {
            event = event.with_actor(id, name.to_string());
        }
        self.audit.record(event).await;
    }

    /// Record an `auth.register_rejected` row for a register request
    /// the public API silently turned into a no-op (already-registered
    /// address, rate limit hit). The HTTP layer always returns
    /// `Ok(())` for these so the only operator-visible signal is this
    /// audit row.
    async fn audit_register_silently_rejected(&self, email: &str, reason: &str) {
        let event =
            AuditEvent::new(AuditEventType::AuthRegisterRejected).with_detail(serde_json::json!({
                "email": email,
                "reason": reason,
            }));
        self.audit.record(event).await;
    }

    /// Record an `auth.register_completed` row when an OTP verify
    /// successfully materialized a fresh `users` row. The actor is
    /// the new user (their first audit row).
    async fn audit_register_completed(&self, user_id: uuid::Uuid, user_name: &str, email: &str) {
        let event = AuditEvent::new(AuditEventType::AuthRegisterCompleted)
            .with_actor(user_id, user_name.to_string())
            .with_detail(serde_json::json!({ "email": email }));
        self.audit.record(event).await;
    }

    /// Record an `auth.verify_failed` row. The HTTP response collapses
    /// every failure into the generic error string; this audit row
    /// preserves the precise reason and (for `bad_code`) the post-bump
    /// remaining-attempt count.
    async fn audit_verify_failed(&self, email: &str, reason: &str, remaining: Option<u32>) {
        let mut detail = serde_json::json!({
            "email": email,
            "reason": reason,
        });
        if let Some(r) = remaining {
            detail["remaining"] = serde_json::json!(r);
        }
        let event = AuditEvent::new(AuditEventType::AuthVerifyFailed).with_detail(detail);
        self.audit.record(event).await;
    }

    // ── Password management ───────────────────────────────────────────

    /// Change the authenticated user's password. Requires the current
    /// password for verification (even though the caller already holds
    /// a valid bearer token — defence in depth against session theft).
    /// Rejects users who do not have a password hash (admin/CLI accounts).
    /// Returns the new bearer token so the caller can continue
    /// authenticated. The old token is invalidated by the rotation.
    pub async fn change_password(
        &self,
        user: &User,
        current_password: &str,
        new_password: &str,
    ) -> Result<String> {
        let hash = self
            .storage
            .get_password_hash(user.id)
            .await?
            .ok_or_else(|| {
                Error::InvalidInput("This account does not use password authentication.".into())
            })?;

        let current_owned = current_password.to_owned();
        let verify =
            tokio::task::spawn_blocking(move || verify_password(&current_owned, &hash)).await;
        match verify {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(Error::Auth("Current password is incorrect.".into())),
            Err(_) => return Err(Error::Internal("verify task panicked".into())),
        }

        validate_password(new_password)?;

        let new_owned = new_password.to_owned();
        let new_hash = tokio::task::spawn_blocking(move || hash_password(&new_owned))
            .await
            .map_err(|_| Error::Internal("hash task panicked".into()))??;

        // Atomically update the password hash AND rotate the bearer
        // token in a single transaction so a crash between the two
        // writes cannot leave the old token valid after a password
        // change.
        let new_token = self
            .storage
            .change_password_and_rotate_token(user.id, &new_hash)
            .await?;

        let email_str = user.email.as_deref().unwrap_or("unknown");
        self.audit
            .record(
                AuditEvent::new(AuditEventType::AuthPasswordChanged)
                    .with_actor(user.id, user.name.clone())
                    .with_detail(serde_json::json!({ "email": email_str })),
            )
            .await;

        Ok(new_token)
    }

    // ── Admin user management ────────────────────────────────────────
    //
    // These three methods back the `/api/admin/users*` endpoints.
    // They live on `AuthService` (not a separate UserService)
    // because the session_enabled bit is an authorization state —
    // conceptually the same domain as login throttling. The HTTP
    // layer is forbidden from reading `state.storage` directly, so
    // every admin surface that touches the `users` table goes
    // through here.

    /// Return every non-guest account row, newest first. The caller
    /// (the admin `list_admin_users` handler) is responsible for the
    /// `is_admin` gate — this method does not re-check it so tests
    /// can drive it directly without a fake actor.
    pub async fn list_accounts(&self) -> Result<Vec<User>> {
        self.storage.list_accounts().await
    }

    pub async fn list_accounts_filtered(&self, filter: &AccountFilter) -> Result<(Vec<User>, i64)> {
        self.storage.list_accounts_filtered(filter).await
    }

    /// Provision a new password-login user directly from the admin
    /// CLI. Validates the password, hashes it on a blocking thread,
    /// and writes the `users` row with `approval_state = 'approved'`
    /// so the new account can create sessions immediately without
    /// admin review. Emits `auth.admin_user_created`. Returns the
    /// created `User` and the freshly minted raw bearer token — the
    /// CLI prints both once, same as the admin bootstrap path.
    ///
    /// This deliberately does NOT share `register` / `verify_otp`'s
    /// enumeration-safe collapsing: the CLI operator wants real error
    /// messages ("email already exists"), not silent 2xx. Conflict
    /// errors propagate from the storage layer verbatim.
    pub async fn admin_create_user(
        &self,
        email: &str,
        name: &str,
        password: &str,
        is_admin: bool,
        session_enabled: bool,
    ) -> Result<(User, String)> {
        validate_password(password)?;

        let password_owned = password.to_owned();
        let hash = tokio::task::spawn_blocking(move || hash_password(&password_owned))
            .await
            .map_err(|_| Error::Internal("hash task panicked".into()))??;

        let email_norm = email.to_lowercase();
        let (user, token) = self
            .storage
            .admin_create_password_user(&email_norm, name, &hash, is_admin, session_enabled)
            .await?;

        self.audit
            .record(
                AuditEvent::new(AuditEventType::AuthAdminUserCreated).with_detail(
                    serde_json::json!({
                        "target_user_id": user.id.to_string(),
                        "target_user_name": user.name,
                        "email": email_norm,
                        "is_admin": is_admin,
                        "session_enabled": session_enabled,
                    }),
                ),
            )
            .await;

        Ok((user, token))
    }

    /// Flip `session_enabled` on the target row and emit the
    /// corresponding audit event. Maps the storage "row not found"
    /// error to `InvalidInput` so the HTTP layer can return 404.
    pub async fn set_session_access(
        &self,
        actor_id: Uuid,
        actor_name: &str,
        target_id: Uuid,
        enabled: bool,
    ) -> Result<User> {
        let updated = self.storage.set_session_enabled(target_id, enabled).await?;
        let event_type = if enabled {
            AuditEventType::AuthUserEnabled
        } else {
            AuditEventType::AuthUserDisabled
        };
        self.audit
            .record(
                AuditEvent::new(event_type)
                    .with_actor(actor_id, actor_name.to_string())
                    .with_detail(serde_json::json!({
                        "target_user_id": updated.id.to_string(),
                        "target_user_name": updated.name,
                    })),
            )
            .await;
        Ok(updated)
    }
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("password hash failed: {e}")))
}

fn verify_password(password: &str, hash: &str) -> Result<()> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| Error::Internal(format!("bad password hash in DB: {e}")))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| Error::Auth("invalid credentials".into()))
}

fn generate_otp() -> String {
    use argon2::password_hash::rand_core::RngCore;
    let limit = 1_000_000u32;
    let threshold = u32::MAX - (u32::MAX % limit);
    let n = loop {
        let candidate = OsRng.next_u32();
        if candidate < threshold {
            break candidate % limit;
        }
    };
    format!("{n:06}")
}

async fn send_otp_email(
    mailer: &lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from: &str,
    to: &str,
    code: &str,
) -> Result<()> {
    use lettre::{AsyncTransport, Message};

    let email = Message::builder()
        .from(
            from.parse()
                .map_err(|e| Error::Internal(format!("bad from address: {e}")))?,
        )
        .to(to
            .parse()
            .map_err(|e| Error::Internal(format!("bad to address: {e}")))?)
        .subject("Your Telepair verification code")
        .body(format!(
            "Your verification code is: {code}\n\n\
             This code expires in {OTP_TTL_MINUTES} minutes.\n\
             If you did not request this, ignore this email."
        ))
        .map_err(|e| Error::Internal(format!("email build failed: {e}")))?;

    mailer
        .send(email)
        .await
        .map_err(|e| Error::ServiceUnavailable(format!("smtp send failed: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use telepair_core::audit::{AuditFilter, AuditSink};
    use telepair_core::storage::SqliteStorage;
    use uuid::Uuid;

    fn make_audit(storage: Arc<SqliteStorage>) -> Arc<AuditSink> {
        Arc::new(AuditSink::new(storage))
    }

    async fn make_service_no_smtp() -> AuthService {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let audit = make_audit(storage.clone());
        AuthService::new(storage, None, audit)
    }

    /// Seed a verified, session-enabled real account whose password
    /// hash matches `password`. Drives the production pending-row
    /// path so the test fixture exercises the same code paths a real
    /// signup would take, then flips `session_enabled` so login tests
    /// stay orthogonal to the admin-approval gate.
    async fn seed_real_account(email: &str, name: &str, password: &str) -> (AuthService, Uuid) {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let hash = hash_password(password).unwrap();
        let expires = Utc::now() + Duration::minutes(15);
        storage
            .upsert_pending_registration(email, name, &hash, "000000", expires)
            .await
            .unwrap();
        let user = match storage
            .verify_pending_registration(email, "000000")
            .await
            .unwrap()
        {
            PendingVerifyResult::Success { user, .. } => user,
            other => panic!("seed: expected Success, got {other:?}"),
        };
        storage.set_session_enabled(user.id, true).await.unwrap();
        let audit = make_audit(storage.clone());
        let svc = AuthService::new(storage, None, audit);
        (svc, user.id)
    }

    #[tokio::test]
    async fn register_no_smtp_returns_service_unavailable() {
        let svc = make_service_no_smtp().await;
        let err = svc
            .register("a@b.com", "password", "alice")
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ServiceUnavailable(_)),
            "expected ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn register_silently_succeeds_when_email_already_registered() {
        // Enumeration safety: a register call against an already-real
        // address must NOT signal that fact to the caller. We can't
        // exercise the SMTP path here, so we go through the storage
        // primitive that backs `get_user_by_email` directly.
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let hash = hash_password("hunter2").unwrap();
        let expires = Utc::now() + Duration::minutes(15);
        storage
            .upsert_pending_registration("dup@x.com", "dup", &hash, "000000", expires)
            .await
            .unwrap();
        storage
            .verify_pending_registration("dup@x.com", "000000")
            .await
            .unwrap();
        let audit = make_audit(storage.clone());
        // SMTP must be configured for `register` to advance past the
        // first check, but we never actually send mail because the
        // already-registered branch returns Ok(()) before reaching
        // the mailer.
        let svc = AuthService::new(
            storage.clone(),
            Some(Arc::new(SmtpConfig {
                host: "localhost".into(),
                port: 25,
                username: "u".into(),
                password: "p".into(),
                from: "noreply@example.com".into(),
            })),
            audit,
        );
        // Returns Ok with no panic, no SMTP traffic, no pending row.
        svc.register("dup@x.com", "anything", "anything")
            .await
            .unwrap();
        // Audit row was written.
        let sink = AuditSink::new(storage.clone());
        let rows = sink.query(AuditFilter::default()).await.unwrap();
        assert!(
            rows.iter()
                .any(|e| e.event_type == AuditEventType::AuthRegisterRejected
                    && e.detail["reason"] == "already_registered"),
            "expected an auth.register_rejected row, got {rows:?}"
        );
    }

    #[tokio::test]
    async fn verify_otp_collapses_failure_modes_to_generic_error() {
        // No pending row exists, so verify_pending_registration
        // returns Expired. The service must collapse that into the
        // generic auth error string — distinguishable shapes would
        // let an attacker enumerate which addresses have a pending
        // row in flight.
        let svc = make_service_no_smtp().await;
        let err = svc.verify_otp("ghost@x.com", "000000").await.unwrap_err();
        match err {
            Error::Auth(msg) => assert_eq!(msg, GENERIC_AUTH_ERROR),
            other => panic!("expected Auth(GENERIC_AUTH_ERROR), got {other:?}"),
        }
    }

    // ── Throttle + audit (Fix #3) ────────────────────────────────────────

    #[tokio::test]
    async fn login_locks_account_after_five_bad_passwords() {
        let (svc, _uid) = seed_real_account("hammer@x.com", "hammer", "correct-horse").await;
        for _ in 0..5 {
            let err = svc.login("hammer@x.com", "wrong-pony").await.unwrap_err();
            assert!(
                matches!(err, Error::Auth(_)),
                "expected Auth on bad password, got {err:?}"
            );
        }
        // Sixth attempt — even with the *correct* password — must be
        // rejected with the generic Auth error because the row is
        // locked. (The locked path no longer surfaces a distinct
        // RateLimited shape: the whole point of the unified error is
        // that an attacker cannot tell "wrong password" from "row
        // currently locked", which would otherwise be a useful signal.)
        let err = svc
            .login("hammer@x.com", "correct-horse")
            .await
            .unwrap_err();
        match err {
            Error::Auth(msg) => assert_eq!(msg, GENERIC_AUTH_ERROR),
            other => panic!("expected Auth(GENERIC_AUTH_ERROR) after 5 strikes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn login_clears_failure_counter_on_success() {
        let (svc, _) = seed_real_account("recover@x.com", "recover", "passw0rd").await;
        // Two strikes, then a success — the counter must reset, so a
        // subsequent run of four strikes does not lock the row.
        for _ in 0..2 {
            let _ = svc.login("recover@x.com", "wrong").await;
        }
        let token = svc.login("recover@x.com", "passw0rd").await.unwrap();
        assert!(!token.is_empty());
        for _ in 0..4 {
            let _ = svc.login("recover@x.com", "wrong").await;
        }
        // Still under threshold (4 < 5), so a good password should
        // still work — proving the success above wiped the slate.
        let token2 = svc.login("recover@x.com", "passw0rd").await.unwrap();
        assert!(!token2.is_empty());
    }

    #[tokio::test]
    async fn login_emits_audit_row_on_failure() {
        let (svc, uid) = seed_real_account("audited@x.com", "audited", "secret").await;
        let _ = svc.login("audited@x.com", "wrong").await;

        // Re-construct an audit sink against the same storage so we
        // can read back what was written. Production goes through
        // the gateway state's shared sink.
        let storage = svc.storage.clone();
        let sink = AuditSink::new(storage);
        let rows = sink.query(AuditFilter::default()).await.unwrap();
        let bad = rows
            .iter()
            .find(|e| e.event_type == AuditEventType::AuthLoginFailed)
            .expect("expected an auth.login_failed row");
        assert_eq!(bad.actor_id, Some(uid));
        assert_eq!(bad.detail["reason"], "bad_password");
        assert_eq!(bad.detail["email"], "audited@x.com");
        assert_eq!(bad.detail["remaining"], 4);
    }

    #[tokio::test]
    async fn login_audits_unknown_email_with_no_actor() {
        let svc = make_service_no_smtp().await;
        let err = svc.login("ghost@x.com", "anything").await.unwrap_err();
        // Same error shape as bad password — no enumeration leak.
        assert!(matches!(err, Error::Auth(_)));

        let storage = svc.storage.clone();
        let sink = AuditSink::new(storage);
        let rows = sink.query(AuditFilter::default()).await.unwrap();
        let row = rows
            .iter()
            .find(|e| e.event_type == AuditEventType::AuthLoginFailed)
            .expect("expected unknown-email audit row");
        assert!(
            row.actor_id.is_none(),
            "unknown-email row must not name a user"
        );
        assert_eq!(row.detail["reason"], "unknown_email");
        assert_eq!(row.detail["email"], "ghost@x.com");
    }

    /// Regression for the email-enumeration timing side channel
    /// (P0-1). Pre-fix, `login("unknown@x", ...)` short-circuited
    /// before touching Argon2 while `login("known@x", "wrong")`
    /// spent ~50 ms hashing, so an unauthenticated attacker could
    /// tell the two apart by latency alone. The fix burns one
    /// Argon2 verify on the unknown-email branch; this test asserts
    /// that the branch now spends real CPU and that its latency is
    /// within a generous tolerance of the wrong-password branch.
    ///
    /// Tolerances are intentionally wide so this test stays
    /// non-flaky on loaded CI runners — the point is to detect
    /// "shield missing" (where `unknown_median` collapses back to
    /// sub-millisecond), not to measure Argon2 down to the
    /// microsecond.
    #[tokio::test]
    async fn login_unknown_email_burns_argon2_for_timing_parity() {
        use std::time::{Duration as StdDuration, Instant};

        let (svc, _) = seed_real_account("real@x.com", "real", "correct-horse").await;

        // Warm up the DUMMY_LOGIN_HASH LazyLock and the SQLite
        // statement cache so the first sample does not carry one-off
        // initialisation cost.
        let _ = svc.login("warmup-ghost@x.com", "anything").await;
        let _ = svc.login("real@x.com", "warmup-wrong").await;

        async fn median_ms(
            svc: &AuthService,
            email_prefix: Option<&str>,
            fixed_email: Option<&str>,
            password: &str,
        ) -> StdDuration {
            let mut samples = Vec::with_capacity(5);
            for i in 0..5 {
                let email = match (email_prefix, fixed_email) {
                    (Some(p), _) => format!("{p}-{i}@x.com"),
                    (_, Some(e)) => e.to_owned(),
                    _ => unreachable!(),
                };
                let start = Instant::now();
                let _ = svc.login(&email, password).await;
                samples.push(start.elapsed());
            }
            samples.sort();
            samples[2]
        }

        let unknown = median_ms(&svc, Some("ghost"), None, "anything").await;
        let known = median_ms(&svc, None, Some("real@x.com"), "wrong-horse").await;

        // Lower bound: Argon2::default() is never cheaper than 10 ms
        // on any CI machine we've seen (debug or release). If this
        // trips, the unknown-email branch is skipping the verify.
        assert!(
            unknown >= StdDuration::from_millis(10),
            "unknown-email branch median was {unknown:?} — expected \
             an Argon2 verify (≥ 10 ms). The timing shield regressed."
        );

        // Parity: the two branches should be within ~3x. We've seen
        // ratios as skewed as 0.6 on a quiet M-series box and 1.8 on
        // a loaded GitHub runner; 0.3–3.0 absorbs the full range
        // while still catching the pre-fix ratio (~0.05).
        let ratio = unknown.as_secs_f64() / known.as_secs_f64();
        assert!(
            (0.3..=3.0).contains(&ratio),
            "unknown-email latency ({unknown:?}) and bad-password \
             latency ({known:?}) should be within 3x — ratio was \
             {ratio:.2}. A tiny ratio means the unknown-email branch \
             regressed to returning without burning Argon2."
        );
    }

    // The second early-return branch (user row exists but carries no
    // password_hash — admin/CLI accounts) routes through the exact
    // same `Self::burn_dummy_verify(password)` call as the unknown-
    // email branch above, so the timing-parity test on that branch
    // exercises the shared helper. A dedicated test here would
    // require a storage primitive to seed a NULL password_hash row
    // that the public API does not expose; we lean on code review +
    // the unknown-email regression instead.

    #[tokio::test]
    async fn login_locked_path_does_not_verify_password() {
        // Lock the row by hand via the storage primitive — five
        // strikes through `login` would also work, but going direct
        // proves that the lockout *check* short-circuits the password
        // verify path even on the very first call.
        let (svc, uid) = seed_real_account("locked@x.com", "locked", "letmein").await;
        for _ in 0..5 {
            svc.storage
                .record_login_failure(uid, Duration::minutes(LOGIN_LOCKOUT_MINUTES))
                .await
                .unwrap();
        }

        // Even the *right* password is rejected with the generic
        // Auth error — the locked path now collapses into the same
        // shape as a bad password to deny the timing/error-message
        // side channel.
        let err = svc.login("locked@x.com", "letmein").await.unwrap_err();
        match err {
            Error::Auth(msg) => assert_eq!(msg, GENERIC_AUTH_ERROR),
            other => panic!("expected Auth(GENERIC_AUTH_ERROR), got {other:?}"),
        }

        // Audit row must mark the reason as "locked" and carry the
        // lockout timestamp.
        let storage = svc.storage.clone();
        let sink = AuditSink::new(storage);
        let rows = sink.query(AuditFilter::default()).await.unwrap();
        let row = rows
            .iter()
            .find(|e| e.detail["reason"] == "locked")
            .expect("expected a 'locked' audit row");
        assert!(row.detail["locked_until"].is_string());
    }

    // ── Change password ─────────────────────────────────────────────

    #[tokio::test]
    async fn change_password_success() {
        let (svc, uid) = seed_real_account("cp@x.com", "cpuser", "old-pass").await;
        let old_token = svc.storage.refresh_user_token(uid).await.unwrap();
        let user = svc.storage.validate_token(&old_token).await.unwrap();

        let new_token = svc
            .change_password(&user, "old-pass", "new-pass")
            .await
            .unwrap();

        // The returned token must be valid and different from the old one.
        assert!(!new_token.is_empty());
        assert_ne!(new_token, old_token, "token must be rotated");
        let validated = svc.storage.validate_token(&new_token).await;
        assert!(validated.is_ok(), "new token must be valid");

        // The old token must be invalidated.
        let old_result = svc.storage.validate_token(&old_token).await;
        assert!(old_result.is_err(), "old token must be invalidated");

        // Verify new password works via login
        let token = svc.login("cp@x.com", "new-pass").await.unwrap();
        assert!(!token.is_empty());

        // Verify old password no longer works
        let err = svc.login("cp@x.com", "old-pass").await.unwrap_err();
        assert!(matches!(err, Error::Auth(_)));

        // Audit row emitted
        let sink = AuditSink::new(svc.storage.clone());
        let rows = sink.query(AuditFilter::default()).await.unwrap();
        let row = rows
            .iter()
            .find(|e| e.event_type == AuditEventType::AuthPasswordChanged)
            .expect("expected an auth.password_changed row");
        assert_eq!(row.actor_id, Some(uid));
        assert_eq!(row.detail["email"], "cp@x.com");
    }

    #[tokio::test]
    async fn change_password_wrong_current_rejects() {
        let (svc, uid) = seed_real_account("cp2@x.com", "cpuser2", "correct").await;
        let user = svc
            .storage
            .validate_token(&svc.storage.refresh_user_token(uid).await.unwrap())
            .await
            .unwrap();

        let err = svc
            .change_password(&user, "wrong-current", "new-pass")
            .await
            .unwrap_err();
        match err {
            Error::Auth(msg) => assert_eq!(msg, "Current password is incorrect."),
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn change_password_no_password_hash_rejects() {
        // Admin/CLI account has no password hash
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let audit = make_audit(storage.clone());
        let svc = AuthService::new(storage.clone(), None, audit);
        let (admin, _) = storage.create_user("admin", true).await.unwrap();

        let err = svc
            .change_password(&admin, "any", "new-pass")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[tokio::test]
    async fn admin_create_user_lowercases_email_and_audits() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let audit = make_audit(storage.clone());
        let svc = AuthService::new(storage.clone(), None, audit);

        let (user, token) = svc
            .admin_create_user("Carol@Example.COM", "carol", "hunter2a", false, true)
            .await
            .expect("admin_create_user must succeed");

        assert_eq!(user.email.as_deref(), Some("carol@example.com"));
        assert!(!token.is_empty());
        assert!(user.session_enabled);
        // Login round-trip with the chosen password must work.
        let login_token = svc
            .login("carol@example.com", "hunter2a")
            .await
            .expect("login with new credentials");
        assert!(!login_token.is_empty());
        // And an audit row landed.
        let rows = AuditSink::new(storage)
            .query(AuditFilter::default())
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|e| e.event_type == AuditEventType::AuthAdminUserCreated),
            "audit log should contain AuthAdminUserCreated: {rows:?}"
        );
    }

    #[tokio::test]
    async fn admin_create_user_rejects_short_password() {
        let svc = make_service_no_smtp().await;
        let err = svc
            .admin_create_user("x@y.com", "x", "short", false, true)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn admin_create_user_duplicate_email_conflicts() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let audit = make_audit(storage.clone());
        let svc = AuthService::new(storage, None, audit);
        svc.admin_create_user("a@b.com", "alice", "hunter2a", false, true)
            .await
            .unwrap();
        let err = svc
            .admin_create_user("a@b.com", "alice2", "hunter2a", false, true)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn admin_create_user_respects_no_session() {
        let svc = make_service_no_smtp().await;
        let (user, _) = svc
            .admin_create_user("p@q.com", "p", "hunter2a", false, false)
            .await
            .unwrap();
        assert!(!user.session_enabled);
    }
}
