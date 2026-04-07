use std::sync::Arc;

use crate::error::{Error, Result};
use crate::session::User;
use crate::storage::{SqliteStorage, Storage};

/// Max attempts when generating a unique guest name before giving up.
/// `users.name` is UNIQUE; with 8 chars of nanoid entropy (~47 bits)
/// a single collision is astronomically rare, but we still bound the
/// retry loop so an unexpected DB state can't spin forever.
const GUEST_NAME_MAX_ATTEMPTS: usize = 5;

pub struct TokenAuthProvider {
    storage: Arc<SqliteStorage>,
}

impl TokenAuthProvider {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub async fn validate(&self, token: &str) -> Result<User> {
        self.storage.validate_token(token).await
    }

    pub async fn setup_initial_admin(&self, name: &str) -> Result<(User, String)> {
        self.storage.create_user(name, true).await
    }

    /// Create a new anonymous guest user whose credentials are bound
    /// to `session_id`. The caller is responsible for returning the
    /// raw token to the client exactly once — the DB only stores its
    /// SHA-256. The guest name is `guest-<nanoid8>`; on the
    /// (vanishingly rare) unique-name collision we retry up to
    /// `GUEST_NAME_MAX_ATTEMPTS` times before surfacing the storage
    /// error.
    ///
    /// `session_id` is stored on the user row so the HTTP layer can
    /// reject account-level routes for scoped users and the WS layer
    /// can reject cross-session joins. **This is the entry point that
    /// closes the invite-link privilege-escalation hole**: without a
    /// bound session, the minted token would be indistinguishable
    /// from a normal non-admin account.
    pub async fn create_guest(&self, session_id: &str) -> Result<(User, String)> {
        let mut last_err = None;
        for _ in 0..GUEST_NAME_MAX_ATTEMPTS {
            let name = format!("guest-{}", nanoid::nanoid!(8));
            match self.storage.create_scoped_guest(&name, session_id).await {
                Ok(pair) => return Ok(pair),
                Err(e) => {
                    // Only retry on the unique-constraint collision.
                    // Anything else (disk full, schema drift, etc.) is
                    // a real failure — surface it immediately so the
                    // caller doesn't waste retries on a doomed DB.
                    if !is_unique_violation(&e) {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            Error::Storage(sqlx::Error::Protocol("exhausted guest name retries".into()))
        }))
    }
}

fn is_unique_violation(err: &Error) -> bool {
    match err {
        Error::Storage(sqlx::Error::Database(db_err)) => {
            // SQLite's UNIQUE-constraint violation is SQLITE_CONSTRAINT_UNIQUE
            // (error code 2067). `is_unique_violation` on the driver error
            // also returns true for this, which is the portable check.
            db_err.is_unique_violation()
        }
        _ => false,
    }
}
