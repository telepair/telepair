use std::sync::Arc;

use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{Duration, Utc};

use telepair_core::error::{Error, Result};
use telepair_core::session::OtpVerifyResult;
use telepair_core::storage::{SqliteStorage, Storage};

const OTP_TTL_MINUTES: i64 = 15;
const OTP_RATE_LIMIT_SECS: i64 = 60;

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
    smtp: Option<Arc<SmtpConfig>>,
}

impl AuthService {
    pub fn new(storage: Arc<SqliteStorage>, smtp: Option<Arc<SmtpConfig>>) -> Self {
        Self { storage, smtp }
    }

    /// Register a new user. Hashes password, creates unverified user row,
    /// and sends an OTP to the email. Returns `ServiceUnavailable` if SMTP
    /// is not configured, `Conflict` if email or display_name is taken.
    pub async fn register(
        &self,
        email: &str,
        password: &str,
        display_name: &str,
    ) -> Result<()> {
        let smtp = self.smtp.as_ref().ok_or_else(|| {
            Error::ServiceUnavailable(
                "This instance has not configured email sending. Contact the administrator."
                    .into(),
            )
        })?;

        // Check if email is already registered before hashing
        if self.storage.get_user_by_email(email).await?.is_some() {
            return Err(Error::Conflict(format!(
                "email already registered: {email}"
            )));
        }

        let hash = hash_password(password)?;
        let user = self
            .storage
            .register_user(email, &hash, display_name)
            .await?;

        // Rate limit: refuse if an OTP was sent within the last 60 seconds
        if let Some(last) = self.storage.latest_otp_created_at(user.id).await? {
            if Utc::now() - last < Duration::seconds(OTP_RATE_LIMIT_SECS) {
                return Err(Error::RateLimited(
                    "Please wait 60 seconds before requesting another code.".into(),
                ));
            }
        }

        let code = generate_otp();
        let expires = Utc::now() + Duration::minutes(OTP_TTL_MINUTES);
        self.storage.create_otp(user.id, &code, expires).await?;

        send_otp_email(smtp, email, &code).await?;
        Ok(())
    }

    /// Verify an OTP code for the given email. Returns the bearer token on
    /// success. Activates the account (sets `verified=true`, issues token).
    pub async fn verify_otp(&self, email: &str, code: &str) -> Result<String> {
        let user = self
            .storage
            .get_user_by_email(email)
            .await?
            .ok_or_else(|| Error::InvalidInput("email not found".into()))?;

        match self.storage.verify_otp(user.id, code).await? {
            OtpVerifyResult::Success => self.storage.activate_user(user.id).await,
            OtpVerifyResult::Failure { remaining } => Err(Error::InvalidInput(format!(
                "Wrong code. {remaining} attempt(s) remaining."
            ))),
            OtpVerifyResult::Locked => Err(Error::InvalidInput(
                "Too many wrong attempts. Please re-register.".into(),
            )),
            OtpVerifyResult::Expired => Err(Error::InvalidInput(
                "Code expired or not found. Please request a new one.".into(),
            )),
        }
    }

    /// Authenticate with email + password. Returns a fresh bearer token.
    /// Returns `Auth` error if credentials are wrong or account is unverified.
    pub async fn login(&self, email: &str, password: &str) -> Result<String> {
        let user = self
            .storage
            .get_user_by_email(email)
            .await?
            .ok_or_else(|| Error::Auth("invalid credentials".into()))?;

        if !user.verified {
            return Err(Error::Auth("account not verified".into()));
        }

        let hash = self
            .storage
            .get_password_hash(user.id)
            .await?
            .ok_or_else(|| Error::Auth("invalid credentials".into()))?;

        verify_password(password, &hash)?;
        self.storage.refresh_user_token(user.id).await
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
    let n = OsRng.next_u32() % 1_000_000;
    format!("{n:06}")
}

async fn send_otp_email(smtp: &SmtpConfig, to: &str, code: &str) -> Result<()> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let email = Message::builder()
        .from(
            smtp.from
                .parse()
                .map_err(|e| Error::Internal(format!("bad from address: {e}")))?,
        )
        .to(to
            .parse()
            .map_err(|e| Error::Internal(format!("bad to address: {e}")))?)
        .subject("Your Telepair verification code")
        .body(format!(
            "Your verification code is: {code}\n\n\
             This code expires in 15 minutes.\n\
             If you did not request this, ignore this email."
        ))
        .map_err(|e| Error::Internal(format!("email build failed: {e}")))?;

    let creds = Credentials::new(smtp.username.clone(), smtp.password.clone());
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)
        .map_err(|e| Error::Internal(format!("smtp relay: {e}")))?
        .port(smtp.port)
        .credentials(creds)
        .build();

    mailer
        .send(email)
        .await
        .map_err(|e| Error::ServiceUnavailable(format!("smtp send failed: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use telepair_core::storage::SqliteStorage;

    async fn make_service_no_smtp() -> AuthService {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        AuthService::new(storage, None)
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
    async fn login_unverified_returns_auth_error() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        // Manually create unverified user
        storage
            .register_user("x@y.com", "hash", "xuser")
            .await
            .unwrap();
        let svc = AuthService::new(storage, None);
        let err = svc.login("x@y.com", "anything").await.unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
    }
}
