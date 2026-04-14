use std::collections::HashMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{Pool, Row, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::audit::{AuditEvent, AuditEventType, AuditFilter};
use crate::error::{Error, Result};
use crate::permission::Role;
use crate::session::{
    CloseReason, CreateUserTargetParams, InputMode, InviteToken, LoginFailureOutcome, Participant,
    PendingVerifyResult, RedeemIdentity, RedeemOutcome, Session, SessionListFilter, SessionStatus,
    User, UserTarget,
};
use crate::storage::{AccountFilter, AccountStatus, Storage};

pub struct SqliteStorage {
    pool: Pool<Sqlite>,
}

impl SqliteStorage {
    pub async fn new(database_url: &str) -> Result<Self> {
        // `busy_timeout` is the production-correctness knob for the
        // multi-statement transactions in `verify_pending_registration`
        // and `redeem_invite`: with WAL we can have many concurrent
        // readers, but two writers racing to promote SHARED → RESERVED
        // would otherwise return `SQLITE_BUSY` immediately. Sleeping
        // for up to 5s gives the loser time to grab the lock once the
        // winner commits, which is the behaviour every other test in
        // this crate already implicitly relies on.
        let options = SqliteConnectOptions::from_str(database_url)?
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePool::connect_with(options).await?;
        let storage = Self { pool };
        storage.run_migrations().await?;
        Ok(storage)
    }

    pub async fn new_memory() -> Result<Self> {
        Self::new("sqlite::memory:").await
    }

    async fn run_migrations(&self) -> Result<()> {
        // WAL mode gives us concurrent readers while a writer is active.
        sqlx::raw_sql("PRAGMA journal_mode=WAL;")
            .execute(&self.pool)
            .await?;

        // Schema is the single source of truth and is applied
        // idempotently on every boot. The SQL file uses
        // `CREATE TABLE/INDEX IF NOT EXISTS` so re-running it against
        // a populated DB is a no-op.
        sqlx::raw_sql(include_str!("../../../../migrations/001_initial.sql"))
            .execute(&self.pool)
            .await?;

        // In-place upgrades from an older v0.1.x DB: `CREATE TABLE IF
        // NOT EXISTS` does not touch an existing table, so columns
        // added after the original v0.1.0 shape need explicit ALTER
        // statements. Each column addition is guarded by a pragma
        // probe so a booted-many-times DB doesn't fail on
        // "duplicate column name".
        self.ensure_column("sessions", "closed_reason", "TEXT")
            .await?;
        self.ensure_column("invite_tokens", "created_at", "TEXT")
            .await?;
        self.ensure_column("users", "email", "TEXT").await?;
        self.ensure_column("users", "password_hash", "TEXT").await?;
        self.ensure_column("users", "verified", "BOOLEAN NOT NULL DEFAULT FALSE")
            .await?;
        self.ensure_column("sessions", "user_target_id", "TEXT")
            .await?;
        // Login throttle (Fix #3): per-user failed-attempt counter and
        // lockout timestamp. Mirrors the OTP 5-strike pattern but lives
        // on the `users` row directly because there is exactly one
        // password per account, so a single counter suffices.
        self.ensure_column("users", "login_failed_count", "INTEGER NOT NULL DEFAULT 0")
            .await?;
        self.ensure_column("users", "login_locked_until", "TEXT")
            .await?;
        // v0.1.2: gate session creation/attach behind an explicit
        // capability bit so a self-registered email account is inert
        // until an admin enables it. Existing rows default TRUE so the
        // upgrade does not lock current users out; the registration
        // path explicitly inserts FALSE for materialized email
        // accounts (see `materialize_pending_registration`).
        self.ensure_column("users", "session_enabled", "BOOLEAN NOT NULL DEFAULT TRUE")
            .await?;

        // Partial unique index on `users.email` — only covers rows
        // where email is non-null. This MUST run after the ALTER
        // above adds the column on legacy DBs, otherwise CREATE INDEX
        // fails with "no such column: email". See the comment in
        // `materialize_pending_registration` for why this guards the
        // registration path from duplicate-email accounts.
        sqlx::raw_sql(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email_unique \
             ON users(email) WHERE email IS NOT NULL",
        )
        .execute(&self.pool)
        .await?;

        // v0.1.2 cleanup: the pre-verification takeover fix moves
        // pending-account state out of `users` into the new
        // `pending_registrations` table. Old DBs may still carry:
        //   1. unverified `users` rows that were inserted by the
        //      now-removed `register_user` path (no token in
        //      circulation, but they tie up the email + display name).
        //   2. an `email_verifications` table that no code references.
        // Pre-1.0 we accept dropping in-flight signups; verified
        // accounts are not touched.
        sqlx::raw_sql("DELETE FROM users WHERE verified = 0 AND email IS NOT NULL")
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql("DROP TABLE IF EXISTS email_verifications")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Retrieve the argon2id hash stored for the given user_id.
    /// Returns `None` if the user does not exist or has no password.
    /// NOT on the `Storage` trait — callers use `AuthService` which
    /// holds the concrete `SqliteStorage` type.
    pub async fn get_password_hash(&self, user_id: Uuid) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
                .bind(user_id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(h,)| h))
    }

    /// Update the password hash for a user. Returns `Error::NotFound`
    /// if no such user exists.
    pub async fn update_password_hash(&self, user_id: Uuid, new_hash: &str) -> Result<()> {
        let rows = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(new_hash)
            .bind(now_rfc3339())
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if rows == 0 {
            return Err(Error::InvalidInput(format!("user {user_id} not found")));
        }
        Ok(())
    }

    /// Atomically update the password hash AND rotate the bearer token
    /// in a single transaction. Returns the new raw token on success.
    /// If either write fails, neither takes effect.
    pub async fn change_password_and_rotate_token(
        &self,
        user_id: Uuid,
        new_hash: &str,
    ) -> Result<String> {
        let now_str = now_rfc3339();
        let uid_str = user_id.to_string();
        let (raw_token, token_hash) = generate_token();

        let mut tx = self.pool.begin().await?;

        let rows = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(new_hash)
            .bind(&now_str)
            .bind(&uid_str)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if rows == 0 {
            return Err(Error::InvalidInput(format!("user {user_id} not found")));
        }

        sqlx::query("UPDATE users SET token_sha256 = ?, updated_at = ? WHERE id = ?")
            .bind(&token_hash)
            .bind(&now_str)
            .bind(&uid_str)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(raw_token)
    }

    /// Disambiguate a `user_targets` UPDATE/DELETE CAS that wrote 0
    /// rows. The CAS in `update_user_target` / `delete_user_target`
    /// folds three distinct rejection reasons into a single miss:
    /// the row doesn't exist, the caller doesn't own it, or it's
    /// referenced by an active session. The first two should map to
    /// `403 PermissionDenied`; the third to `409 Conflict` with a
    /// human-readable "close the session first" message so the Web
    /// UI can render a meaningful hint instead of a generic error.
    async fn classify_user_target_mutation_failure(&self, id: &str, user_id: Uuid) -> Error {
        // Does the row belong to this user at all?
        let owned: Option<i64> = match sqlx::query_scalar(
            "SELECT 1 FROM user_targets WHERE id = ? AND user_id = ? LIMIT 1",
        )
        .bind(id)
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await
        {
            Ok(v) => v,
            Err(e) => return Error::Storage(e),
        };
        if owned.is_none() {
            return Error::PermissionDenied(format!(
                "target {id} not found or not owned by caller"
            ));
        }
        // Row exists and is owned — the only remaining reason the
        // CAS could have written 0 rows is the active-session guard.
        Error::Conflict(format!(
            "target {id} is in use by an active session; close the session first"
        ))
    }

    /// Idempotently add `column` to `table` if it does not already
    /// exist. Uses `pragma_table_info` to probe — cheaper than
    /// parsing `information_schema` in SQLite and works without
    /// relying on ALTER-TABLE error codes.
    ///
    /// `table`, `column`, and `sql_type` are **interpolated into SQL
    /// without escaping**. They MUST be hardcoded constants at every
    /// call site — never user input. The argument type is `&str`
    /// rather than a stricter type because the callers are inside
    /// this module and the string is always a literal.
    async fn ensure_column(&self, table: &str, column: &str, sql_type: &str) -> Result<()> {
        let probe = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?");
        let exists: Option<i64> = sqlx::query_scalar(&probe)
            .bind(column)
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_some() {
            return Ok(());
        }
        let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {sql_type}");
        sqlx::raw_sql(&alter).execute(&self.pool).await?;
        Ok(())
    }
}

fn parse_uuid(s: String) -> Result<Uuid> {
    s.parse()
        .map_err(|e| Error::InvalidInput(format!("invalid uuid: {e}")))
}

fn parse_datetime(s: String) -> Result<chrono::DateTime<Utc>> {
    s.parse()
        .map_err(|e| Error::InvalidInput(format!("invalid timestamp: {e}")))
}

/// Canonical RFC3339 format used by every timestamp column in the DB.
/// Centralizes the format choice so a future switch (e.g. sub-second
/// precision) lands in one place instead of drifting across write sites.
fn rfc3339(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn now_rfc3339() -> String {
    rfc3339(Utc::now())
}

/// Column list shared by every `SELECT … FROM users` that feeds
/// `row_to_user`. Centralised so a new column only needs one change.
const USER_COLS: &str =
    "id, name, is_admin, scoped_session_id, email, session_enabled, created_at, updated_at";

fn row_to_user(r: &SqliteRow) -> Result<User> {
    Ok(User {
        id: parse_uuid(r.get("id"))?,
        name: r.get("name"),
        is_admin: r.get("is_admin"),
        scoped_session_id: r.get("scoped_session_id"),
        email: r.try_get("email").ok().flatten(),
        // `session_enabled` defaults TRUE on legacy rows (the column
        // ALTER specifies `DEFAULT TRUE`); the `try_get` fallback to
        // `true` covers tests that select a narrower column set.
        session_enabled: r.try_get::<bool, _>("session_enabled").unwrap_or(true),
        created_at: parse_datetime(r.get("created_at"))?,
        updated_at: parse_datetime(r.get("updated_at"))?,
    })
}

/// Parse an optional RFC3339 timestamp column. Returns `Ok(None)` if the
/// column was SQL NULL; returns `Err` if the column held a non-null value
/// that failed to parse. The old code swallowed parse errors with
/// `and_then(|s| s.parse().ok())`, which silently converted a corrupt
/// `closed_at` / `left_at` / `expires_at` into `None` — downstream code
/// then thought the row was "still active" / "never expires", which is
/// exactly the kind of subtle data-loss bug we want to fail loudly on.
fn parse_optional_datetime(
    raw: Option<String>,
    column: &'static str,
) -> Result<Option<chrono::DateTime<Utc>>> {
    match raw {
        None => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|e| Error::InvalidInput(format!("invalid {column}: {e}"))),
    }
}

fn row_to_session(r: &SqliteRow) -> Result<Session> {
    // `closed_reason` is nullable (v0.1.0 rows + still-active rows
    // both read as NULL). Unknown string values become
    // `Error::InvalidInput` so a corrupt row fails loudly rather
    // than silently collapsing to `None` — same policy as the other
    // parse helpers in this file.
    let closed_reason: Option<String> = r.get("closed_reason");
    let closed_reason = match closed_reason {
        None => None,
        Some(s) => Some(
            CloseReason::from_str(&s)
                .map_err(|e| Error::InvalidInput(format!("invalid closed_reason: {e}")))?,
        ),
    };
    Ok(Session {
        id: r.get("id"),
        owner_id: parse_uuid(r.get("owner_id"))?,
        target_name: r.get("target_name"),
        input_mode: r
            .get::<String, _>("input_mode")
            .parse()
            .map_err(Error::InvalidInput)?,
        status: r
            .get::<String, _>("status")
            .parse()
            .map_err(Error::InvalidInput)?,
        created_at: parse_datetime(r.get("created_at"))?,
        closed_at: parse_optional_datetime(r.get("closed_at"), "closed_at")?,
        closed_reason,
        user_target_id: r.try_get("user_target_id").ok().flatten(),
    })
}

fn row_to_participant(r: &SqliteRow) -> Result<Participant> {
    Ok(Participant {
        session_id: r.get("session_id"),
        user_id: parse_uuid(r.get("user_id"))?,
        role: r
            .get::<String, _>("role")
            .parse()
            .map_err(Error::InvalidInput)?,
        joined_at: parse_datetime(r.get("joined_at"))?,
        left_at: parse_optional_datetime(r.get("left_at"), "left_at")?,
    })
}

fn row_to_invite(r: &SqliteRow) -> Result<InviteToken> {
    Ok(InviteToken {
        token_sha256: r.get("token_sha256"),
        session_id: r.get("session_id"),
        role: r
            .get::<String, _>("role")
            .parse()
            .map_err(Error::InvalidInput)?,
        max_uses: r.get("max_uses"),
        used_count: r.get("used_count"),
        expires_at: parse_optional_datetime(r.get("expires_at"), "expires_at")?,
        // `created_at` is nullable: v0.1.0 rows are `NULL`, new rows
        // are always populated by `create_invite`.
        created_at: parse_optional_datetime(r.get("created_at"), "created_at")?,
    })
}

fn row_to_user_target(r: &SqliteRow) -> Result<UserTarget> {
    let args_json: String = r.get("args");
    let env_json: String = r.get("env");
    let tags_json: String = r.get("tags");
    Ok(UserTarget {
        id: r.get("id"),
        user_id: parse_uuid(r.get("user_id"))?,
        name: r.get("name"),
        display: r.get("display"),
        command: r.get("command"),
        args: serde_json::from_str(&args_json)
            .map_err(|e| Error::InvalidInput(format!("invalid args json: {e}")))?,
        env: serde_json::from_str(&env_json)
            .map_err(|e| Error::InvalidInput(format!("invalid env json: {e}")))?,
        tags: serde_json::from_str(&tags_json)
            .map_err(|e| Error::InvalidInput(format!("invalid tags json: {e}")))?,
        created_at: parse_datetime(r.get("created_at"))?,
        updated_at: parse_datetime(r.get("updated_at"))?,
    })
}

fn row_to_audit_event(r: &SqliteRow) -> Result<AuditEvent> {
    // `detail` is stored as JSON text and round-trips through
    // `serde_json`. SQL NULL maps back to `Value::Null` — the
    // same sentinel the in-memory representation uses for "no
    // extra data" — so callers never have to special-case the
    // storage layer's missing-vs-null distinction.
    let detail_raw: Option<String> = r.get("detail");
    let detail = match detail_raw {
        None => serde_json::Value::Null,
        Some(s) => serde_json::from_str(&s)
            .map_err(|e| Error::InvalidInput(format!("invalid audit detail json: {e}")))?,
    };
    let actor_id: Option<String> = r.get("actor_id");
    let actor_id = match actor_id {
        None => None,
        Some(s) => Some(parse_uuid(s)?),
    };
    let event_type: String = r.get("event_type");
    let event_type =
        AuditEventType::from_str(&event_type).map_err(|e| Error::InvalidInput(e.to_string()))?;
    Ok(AuditEvent {
        id: Some(r.get::<i64, _>("id")),
        ts: parse_datetime(r.get("ts"))?,
        actor_id,
        actor_name: r.get("actor_name"),
        event_type,
        session_id: r.get("session_id"),
        detail,
    })
}

/// Compute SHA-256 hex digest of a raw token for O(1) indexed lookup.
fn token_sha256(raw: &str) -> String {
    let hash = Sha256::digest(raw.as_bytes());
    hex::encode(hash)
}

/// Generate a new token, returning (raw, sha256_hex). The sha256 digest
/// is the only server-side representation — we don't keep a bcrypt hash
/// alongside it because every lookup already goes through the indexed
/// sha256 column. Raw tokens are 32-char nanoids (≈190 bits of entropy),
/// so the second hash added cost without raising the security floor.
fn generate_token() -> (String, String) {
    let raw = nanoid::nanoid!(32);
    let sha256_hex = token_sha256(&raw);
    (raw, sha256_hex)
}

fn check_invite_validity(invite: &InviteToken) -> Result<()> {
    if let Some(expires_at) = invite.expires_at
        && expires_at < Utc::now()
    {
        return Err(Error::InvalidInput("invite token has expired".into()));
    }
    if invite.used_count >= invite.max_uses {
        return Err(Error::InvalidInput(
            "invite token has been fully used".into(),
        ));
    }
    Ok(())
}

impl SqliteStorage {
    /// Resolve a raw token via SHA-256 indexed lookup. Returns `None` on
    /// miss so callers pick the right error variant (user tokens → 401,
    /// invite tokens → 400).
    ///
    /// `table` is interpolated into SQL — it MUST be a hardcoded constant,
    /// never user input.
    async fn lookup_by_token<T>(
        &self,
        table: &str,
        raw_token: &str,
        map_row: impl Fn(&SqliteRow) -> Result<T>,
    ) -> Result<Option<T>> {
        let sha256_hex = token_sha256(raw_token);
        let query = format!("SELECT * FROM {table} WHERE token_sha256 = ?");
        sqlx::query(&query)
            .bind(&sha256_hex)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| map_row(&row))
            .transpose()
    }

    /// Fetch a user by id. Test-only: production auth always validates by
    /// token, so there is no production caller for this lookup — it lives
    /// here as an inherent method (not on the `Storage` trait) so test
    /// assertions can verify inserts landed.
    pub async fn get_user(&self, id: Uuid) -> Result<Option<User>> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM users WHERE id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;

        row.map(|r| row_to_user(&r)).transpose()
    }

    /// Shared implementation of account creation, parameterised over
    /// `is_admin` and the optional `scoped_session_id`. `create_user`
    /// always passes `None`; `create_scoped_guest` passes
    /// `Some(session_id)`. Keeping one INSERT statement means every
    /// path agrees on column ordering and default handling.
    ///
    /// Both call sites mint a verified, session-enabled row: admin
    /// accounts (CLI bootstrap) need full access, and invite-minted
    /// scoped guests can attach to exactly one session anyway (the
    /// `scoped_session_id` gate carries the real authority — the
    /// `session_enabled` bit is left TRUE so the WS attach handshake
    /// does not double-reject them on the wrong axis).
    async fn insert_user(
        &self,
        name: &str,
        is_admin: bool,
        scoped_session_id: Option<&str>,
    ) -> Result<(User, String)> {
        let id = Uuid::new_v4();
        let (token, sha256_hex) = generate_token();
        let now = Utc::now();
        let now_str = rfc3339(now);

        sqlx::query(
            "INSERT INTO users \
               (id, name, token_sha256, is_admin, scoped_session_id, \
                verified, session_enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, TRUE, TRUE, ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(&sha256_hex)
        .bind(is_admin)
        .bind(scoped_session_id)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await?;

        let user = User {
            id,
            name: name.into(),
            is_admin,
            scoped_session_id: scoped_session_id.map(|s| s.to_owned()),
            email: None,
            session_enabled: true,
            created_at: now,
            updated_at: now,
        };
        Ok((user, token))
    }
}

impl Storage for SqliteStorage {
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)> {
        // Unscoped path: real account, full route access (subject to
        // `is_admin`). `scoped_session_id` stays NULL.
        self.insert_user(name, is_admin, None).await
    }

    async fn create_scoped_guest(&self, name: &str, session_id: &str) -> Result<(User, String)> {
        // Scoped path: guest bound to this one session, never an admin.
        // The HTTP and WS layers enforce the scope at request time.
        self.insert_user(name, false, Some(session_id)).await
    }

    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM users WHERE name = ?"))
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        row.map(|r| row_to_user(&r)).transpose()
    }

    async fn validate_token(&self, token: &str) -> Result<User> {
        self.lookup_by_token("users", token, row_to_user)
            .await?
            .ok_or_else(|| Error::Auth("invalid token".into()))
    }

    async fn create_session_with_owner(
        &self,
        owner_id: Uuid,
        target_name: &str,
        input_mode: InputMode,
        user_target_id: Option<&str>,
    ) -> Result<Session> {
        let id = nanoid::nanoid!(10);
        let now = Utc::now();
        let now_str = rfc3339(now);
        let owner_str = owner_id.to_string();

        // One transaction covers both INSERTs: either the caller gets a
        // session row with its owner participant, or neither exists.
        // Previously these were two separate calls — a failure between
        // them (DB disconnect, foreign-key violation, crash) left an
        // owner-less session that the owner couldn't rejoin because
        // ws.rs's NOT_PARTICIPANT check walks the participants table.
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO sessions \
             (id, owner_id, target_name, input_mode, status, created_at, user_target_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&owner_str)
        .bind(target_name)
        .bind(input_mode.as_str())
        .bind(SessionStatus::Active.as_str())
        .bind(&now_str)
        .bind(user_target_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO participants (session_id, user_id, role, joined_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&owner_str)
        .bind(Role::Owner.as_str())
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Session {
            id,
            owner_id,
            target_name: target_name.into(),
            input_mode,
            status: SessionStatus::Active,
            created_at: now,
            closed_at: None,
            closed_reason: None,
            user_target_id: user_target_id.map(|s| s.to_string()),
        })
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        row.map(|r| row_to_session(&r)).transpose()
    }

    async fn close_session(&self, id: &str, reason: CloseReason) -> Result<()> {
        // Close the session AND stamp left_at on every still-active
        // participant in one transaction. The two used to be split:
        // `close_session` only touched the sessions row, and the WS
        // handler eagerly wrote `left_at` the moment a socket closed.
        // That eager write caused invitee reconnects to fail — the
        // Reaper grace period kept the in-memory session alive so the
        // client was still welcome back, but the participant row had
        // already been marked "left", so the WS handshake's
        // NOT_PARTICIPANT check rejected them. Folding the cleanup
        // into the single close path keeps both sides in sync: while
        // the session is alive, participants stay rejoinable; once
        // it's closed, everybody is consistently marked gone.
        let now_str = now_rfc3339();
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            "UPDATE sessions SET status = ?, closed_at = ?, closed_reason = ? \
             WHERE id = ? AND status = ?",
        )
        .bind(SessionStatus::Closed.as_str())
        .bind(&now_str)
        .bind(reason.as_str())
        .bind(id)
        .bind(SessionStatus::Active.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            // Nothing to do — either the row doesn't exist or another
            // caller already closed it. Roll back the empty tx and
            // propagate NotFound so idempotent close still behaves
            // like "not active anymore".
            return Err(Error::SessionNotFound(id.to_string()));
        }

        sqlx::query(
            "UPDATE participants SET left_at = ? \
             WHERE session_id = ? AND left_at IS NULL",
        )
        .bind(&now_str)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn list_sessions_for_user(
        &self,
        user_id: Uuid,
        filter: SessionListFilter,
    ) -> Result<Vec<Session>> {
        // History-aware query: unlike the pre-0.1.1 implementation,
        // the LEFT JOIN no longer filters on `p.left_at IS NULL`.
        // That predicate made sense for "what am I currently in" but
        // actively hid every session the user participated in and
        // then left — exactly the rows the Closed tab needs to show.
        // The WHERE clause still requires either ownership or
        // participant membership, so strangers never leak in.
        //
        // The SQL is assembled from hardcoded fragments; every `?`
        // placeholder is bound through sqlx, and no caller-supplied
        // string ever touches the query text. Anything added here
        // must preserve that invariant.
        let uid = user_id.to_string();
        let mut sql = String::from(
            "SELECT DISTINCT s.* FROM sessions s \
             LEFT JOIN participants p ON p.session_id = s.id AND p.user_id = ? \
             WHERE (s.owner_id = ? OR p.user_id IS NOT NULL)",
        );
        if filter.status.is_some() {
            sql.push_str(" AND s.status = ?");
        }
        if filter.target_name.is_some() {
            sql.push_str(" AND s.target_name = ?");
        }
        sql.push_str(" ORDER BY s.created_at DESC");
        // SQLite rejects `OFFSET` without an accompanying `LIMIT`. When
        // the caller wants "skip N, return everything after", emit the
        // documented `LIMIT -1` sentinel so the query still parses.
        // Without this, `GET /api/sessions?offset=25` would 500.
        if filter.limit.is_some() {
            sql.push_str(" LIMIT ?");
        } else if filter.offset > 0 {
            sql.push_str(" LIMIT -1");
        }
        if filter.offset > 0 {
            sql.push_str(" OFFSET ?");
        }

        let mut q = sqlx::query(&sql).bind(&uid).bind(&uid);
        if let Some(status) = filter.status {
            q = q.bind(status.as_str().to_string());
        }
        if let Some(target) = filter.target_name.as_ref() {
            q = q.bind(target.clone());
        }
        if let Some(limit) = filter.limit {
            q = q.bind(limit);
        }
        if filter.offset > 0 {
            q = q.bind(filter.offset);
        }

        let rows = q.fetch_all(&self.pool).await?;

        rows.into_iter()
            .map(|r| row_to_session(&r))
            .collect::<Result<Vec<_>>>()
    }

    async fn list_all_sessions(&self, filter: SessionListFilter) -> Result<Vec<Session>> {
        // Same shape as `list_sessions_for_user` minus the
        // ownership/participant WHERE — `SessionService` only calls
        // this after `User::is_admin` is true. Keeping the two
        // queries as separate SQL strings is deliberate: folding an
        // `OR ? = 1 /* is_admin */` branch into the user-scoped
        // query would blur the "did a non-admin accidentally get
        // admin rows" invariant that code review relies on.
        //
        // The SQL is assembled from hardcoded fragments; every `?`
        // placeholder is bound through sqlx, and no caller-supplied
        // string ever touches the query text.
        let mut sql = String::from("SELECT s.* FROM sessions s WHERE 1 = 1");
        if filter.status.is_some() {
            sql.push_str(" AND s.status = ?");
        }
        if filter.target_name.is_some() {
            sql.push_str(" AND s.target_name = ?");
        }
        sql.push_str(" ORDER BY s.created_at DESC");
        // Same `LIMIT -1` sentinel as `list_sessions_for_user` so
        // `offset` without `limit` still parses under SQLite.
        if filter.limit.is_some() {
            sql.push_str(" LIMIT ?");
        } else if filter.offset > 0 {
            sql.push_str(" LIMIT -1");
        }
        if filter.offset > 0 {
            sql.push_str(" OFFSET ?");
        }

        let mut q = sqlx::query(&sql);
        if let Some(status) = filter.status {
            q = q.bind(status.as_str().to_string());
        }
        if let Some(target) = filter.target_name.as_ref() {
            q = q.bind(target.clone());
        }
        if let Some(limit) = filter.limit {
            q = q.bind(limit);
        }
        if filter.offset > 0 {
            q = q.bind(filter.offset);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| row_to_session(&r))
            .collect::<Result<Vec<_>>>()
    }

    async fn close_stale_sessions(&self, reason: CloseReason) -> Result<u64> {
        // Boot-time recovery: an unclean shutdown can leave "active"
        // sessions in the DB that no longer map to any running PTY.
        // Close them AND settle their participants in the same tx
        // so `left_at` stays consistent with the sessions row.
        //
        // We use `UPDATE … RETURNING id` to capture the exact set of
        // rows this sweep just closed, then feed those ids into the
        // participants UPDATE. The older implementation tried to
        // locate "the sessions we just closed" via
        // `WHERE status='closed' AND closed_at = ?`, which conflated
        // "just closed in this sweep" with "historically closed at
        // the same timestamp string" — any historical row whose
        // `closed_at` happened to collide with the sweep's `now_str`
        // would have its participants' `left_at` silently rewritten
        // to the sweep time, clobbering real history. RETURNING
        // removes the time-equality heuristic entirely.
        let now_str = now_rfc3339();
        let mut tx = self.pool.begin().await?;

        let closed_ids: Vec<String> = sqlx::query_scalar(
            "UPDATE sessions SET status = ?, closed_at = ?, closed_reason = ? \
             WHERE status = ? RETURNING id",
        )
        .bind(SessionStatus::Closed.as_str())
        .bind(&now_str)
        .bind(reason.as_str())
        .bind(SessionStatus::Active.as_str())
        .fetch_all(&mut *tx)
        .await?;

        if !closed_ids.is_empty() {
            // Bind each id individually — SQLite parameter limits (999
            // by default) are well above any realistic live-session
            // count on a single node, so one statement is fine. A
            // node with enough stale sessions to blow the parameter
            // limit has bigger problems than this sweep.
            let placeholders = vec!["?"; closed_ids.len()].join(",");
            let sql = format!(
                "UPDATE participants SET left_at = ? \
                 WHERE left_at IS NULL AND session_id IN ({placeholders})"
            );
            let mut q = sqlx::query(&sql).bind(&now_str);
            for id in &closed_ids {
                q = q.bind(id);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(closed_ids.len() as u64)
    }

    async fn count_active_sessions_per_target(&self) -> Result<HashMap<String, u32>> {
        // One indexed GROUP BY over `sessions`. Targets with no
        // active rows are omitted from the map — callers look up
        // absent targets as zero. `COUNT(*)` comes back as i64 from
        // SQLite, clamped to `u32` because a single node with 4B
        // live PTYs is a fantasy and a negative row count is a bug.
        let rows = sqlx::query(
            "SELECT target_name, COUNT(*) AS n FROM sessions \
             WHERE status = ? GROUP BY target_name",
        )
        .bind(SessionStatus::Active.as_str())
        .fetch_all(&self.pool)
        .await?;

        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get("target_name")?;
            let n: i64 = row.try_get("n")?;
            let clamped = n.clamp(0, u32::MAX as i64) as u32;
            out.insert(name, clamped);
        }
        Ok(out)
    }

    async fn upsert_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        role: Role,
    ) -> Result<Participant> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO participants (session_id, user_id, role, joined_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT (session_id, user_id) DO UPDATE SET role = excluded.role, left_at = NULL",
        )
        .bind(session_id)
        .bind(user_id.to_string())
        .bind(role.as_str())
        .bind(rfc3339(now))
        .execute(&self.pool)
        .await?;

        Ok(Participant {
            session_id: session_id.into(),
            user_id,
            role,
            joined_at: now,
            left_at: None,
        })
    }

    async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>> {
        let rows =
            sqlx::query("SELECT * FROM participants WHERE session_id = ? AND left_at IS NULL")
                .bind(session_id)
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|r| row_to_participant(&r))
            .collect::<Result<Vec<_>>>()
    }

    async fn find_active_participant_role(
        &self,
        session_id: &str,
        user_id: Uuid,
    ) -> Result<Option<Role>> {
        // Single SELECT-JOIN: both predicates — `p.left_at IS NULL`
        // and `s.status = 'active'` — are evaluated against the same
        // MVCC snapshot, so a concurrent `close_session` cannot land
        // partially visible between them. The two-query version this
        // replaces (get_session + list_participants) had a narrow but
        // real TOCTOU window in which a close could commit between
        // the status read and the participant read. The single-query
        // shape does not fix the *subsequent* race between this query
        // and any action the caller takes on the returned value —
        // callers that need fully-atomic redemption must wrap their
        // own write in the same transaction (see
        // `Storage::redeem_invite`).
        let row = sqlx::query(
            "SELECT p.role FROM participants p \
             JOIN sessions s ON s.id = p.session_id \
             WHERE p.session_id = ? AND p.user_id = ? \
               AND p.left_at IS NULL AND s.status = 'active'",
        )
        .bind(session_id)
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => {
                let role: Role = r
                    .get::<String, _>("role")
                    .parse()
                    .map_err(Error::InvalidInput)?;
                Ok(Some(role))
            }
            None => Ok(None),
        }
    }

    async fn create_invite(
        &self,
        session_id: &str,
        role: Role,
        max_uses: i32,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<(InviteToken, String)> {
        if max_uses < 1 {
            return Err(Error::InvalidInput("max_uses must be at least 1".into()));
        }
        let (raw_token, sha256_hex) = generate_token();
        let created_at = Utc::now();

        sqlx::query(
            "INSERT INTO invite_tokens (token_sha256, session_id, role, max_uses, used_count, expires_at, created_at) \
             VALUES (?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(&sha256_hex)
        .bind(session_id)
        .bind(role.as_str())
        .bind(max_uses)
        .bind(expires_at.map(rfc3339))
        .bind(rfc3339(created_at))
        .execute(&self.pool)
        .await?;

        let invite = InviteToken {
            token_sha256: sha256_hex,
            session_id: session_id.into(),
            role,
            max_uses,
            used_count: 0,
            expires_at,
            created_at: Some(created_at),
        };
        Ok((invite, raw_token))
    }

    async fn list_invites_for_session(&self, session_id: &str) -> Result<Vec<InviteToken>> {
        // Newest-first so the UI can render a chronological feed
        // without sorting on the client. Exhausted / revoked rows
        // disappear from here via `revoke_invite`; the service
        // layer decides whether to hide "fully used" rows.
        let rows = sqlx::query(
            "SELECT * FROM invite_tokens WHERE session_id = ? \
             ORDER BY COALESCE(created_at, '') DESC, token_sha256 ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_invite).collect()
    }

    async fn revoke_invite(&self, token_sha256: &str) -> Result<()> {
        // Hard delete: a revoked invite has no use to the caller and
        // nothing downstream references it by its PK (participants
        // are keyed by session+user, not by invite). Returning
        // `Error::InvalidInput` on miss mirrors `find_invite`'s
        // "unknown token" behavior so the HTTP layer maps both to
        // 400 without a separate branch.
        let result = sqlx::query("DELETE FROM invite_tokens WHERE token_sha256 = ?")
            .bind(token_sha256)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidInput("invite token not found".into()));
        }
        Ok(())
    }

    async fn find_invite(&self, token: &str) -> Result<InviteToken> {
        self.lookup_by_token("invite_tokens", token, row_to_invite)
            .await?
            .ok_or_else(|| Error::InvalidInput("invalid invite token".into()))
    }

    async fn find_invite_by_sha256(&self, token_sha256: &str) -> Result<Option<InviteToken>> {
        // Direct PK lookup — the SHA-256 column is the primary key,
        // so this is an indexed O(1) read regardless of how many
        // invites a session accumulates.
        let row = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 = ?")
            .bind(token_sha256)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_invite).transpose()
    }

    async fn redeem_invite(
        &self,
        token: &str,
        identity: RedeemIdentity<'_>,
    ) -> Result<RedeemOutcome> {
        // One transaction covers: (1) atomic consume that also
        // requires `sessions.status = 'active'` in the WHERE clause
        // (closes the TOCTOU window the pre-0.1.2 code had between a
        // service-layer "session still active?" pre-check and the
        // participant write), (2) optional guest user INSERT, (3)
        // participant upsert. Any step can fail and the rolled-back
        // transaction leaves `used_count` untouched — critical for
        // the retry story on UNIQUE(name) guest collisions.
        let sha256_hex = token_sha256(token);
        let now = Utc::now();
        let now_str = rfc3339(now);

        let mut tx = self.pool.begin().await?;

        // The Existing-identity branch adds a `NOT EXISTS(active
        // participant for same user)` clause to the UPDATE WHERE.
        // Without it, two concurrent redeems by the same already-
        // authenticated caller (e.g. a double-clicked share link)
        // could each sail past the service-layer pre-check and
        // each bump `used_count`, even though the participant-row
        // upsert collapses them into a single membership — leaking
        // seats on multi-use invites and producing a silent
        // `used_count / participants` mismatch. The NewGuest
        // branch cannot race with itself (the guest row does not
        // exist until this transaction commits) and is left with
        // the simpler three-predicate guard.
        let update = match identity {
            RedeemIdentity::Existing(user_id) => {
                sqlx::query(
                    "UPDATE invite_tokens SET used_count = used_count + 1 \
                     WHERE token_sha256 = ? \
                       AND used_count < max_uses \
                       AND (expires_at IS NULL OR expires_at > ?) \
                       AND EXISTS ( \
                           SELECT 1 FROM sessions \
                           WHERE id = invite_tokens.session_id AND status = ? \
                       ) \
                       AND NOT EXISTS ( \
                           SELECT 1 FROM participants \
                           WHERE session_id = invite_tokens.session_id \
                             AND user_id = ? \
                             AND left_at IS NULL \
                       )",
                )
                .bind(&sha256_hex)
                .bind(&now_str)
                .bind(SessionStatus::Active.as_str())
                .bind(user_id.to_string())
                .execute(&mut *tx)
                .await?
            }
            RedeemIdentity::NewGuest { .. } => {
                sqlx::query(
                    "UPDATE invite_tokens SET used_count = used_count + 1 \
                     WHERE token_sha256 = ? \
                       AND used_count < max_uses \
                       AND (expires_at IS NULL OR expires_at > ?) \
                       AND EXISTS ( \
                           SELECT 1 FROM sessions \
                           WHERE id = invite_tokens.session_id AND status = ? \
                       )",
                )
                .bind(&sha256_hex)
                .bind(&now_str)
                .bind(SessionStatus::Active.as_str())
                .execute(&mut *tx)
                .await?
            }
        };

        if update.rows_affected() == 0 {
            // Zero-row update means one of the WHERE predicates
            // failed. Do diagnostic SELECTs (still inside the tx,
            // so they see the same snapshot) to pick the right
            // branch. Precedence matches the pre-0.1.2 service-
            // layer checks: unknown token → 400, session
            // closed/gone → 410/404, already-member (Existing
            // only) → idempotent no-op, otherwise expired /
            // exhausted → 400.
            let invite_row = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 = ?")
                .bind(&sha256_hex)
                .fetch_optional(&mut *tx)
                .await?;
            let Some(invite_row) = invite_row else {
                return Err(Error::InvalidInput("invalid invite token".into()));
            };
            let invite = row_to_invite(&invite_row)?;

            let session_row = sqlx::query("SELECT status FROM sessions WHERE id = ?")
                .bind(&invite.session_id)
                .fetch_optional(&mut *tx)
                .await?;
            match session_row {
                None => return Err(Error::SessionNotFound(invite.session_id)),
                Some(row) => {
                    let status: String = row.try_get("status")?;
                    if status != SessionStatus::Active.as_str() {
                        return Err(Error::SessionClosed(invite.session_id));
                    }
                }
            }

            // Idempotent short path: only reachable via the
            // Existing-identity branch (the NewGuest UPDATE has no
            // participants predicate). If the caller is already an
            // active participant of this session, return a no-op
            // outcome so the service layer can skip its audit
            // writes — the original `ParticipantJoined` row is
            // still the source of truth for "when did they join".
            // `used_count` is NOT bumped: the returned `invite`
            // carries the pre-call row as-is.
            if let RedeemIdentity::Existing(user_id) = identity {
                let existing = sqlx::query(
                    "SELECT u.name FROM participants p \
                     JOIN users u ON u.id = p.user_id \
                     WHERE p.session_id = ? AND p.user_id = ? AND p.left_at IS NULL",
                )
                .bind(&invite.session_id)
                .bind(user_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
                if let Some(row) = existing {
                    let user_name: String = row.try_get("name")?;
                    tx.commit().await?;
                    return Ok(RedeemOutcome {
                        invite,
                        user_id,
                        user_name,
                        issued_token: None,
                        was_already_member: true,
                    });
                }
            }

            // Session is alive, caller is not already a member —
            // so the failure must be expiry or exhaustion.
            // `check_invite_validity` returns the precise message
            // for each.
            check_invite_validity(&invite)?;
            return Err(Error::InvalidInput(
                "invite token has been fully used".into(),
            ));
        }

        // Re-read the updated invite row to return authoritative
        // post-consume state (used_count, role, expires_at) without
        // forcing the caller to track the increment by hand.
        let invite_row = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 = ?")
            .bind(&sha256_hex)
            .fetch_one(&mut *tx)
            .await?;
        let invite = row_to_invite(&invite_row)?;

        // Resolve the identity. `Existing` paths look up the current
        // `users.name` so the caller's audit row reflects the truth
        // in storage even if they passed a stale `User` struct.
        // `NewGuest` paths INSERT a scoped-session guest — the raw
        // bearer is surfaced once in `RedeemOutcome.issued_token`.
        let (user_id, user_name, issued_token) = match identity {
            RedeemIdentity::Existing(id) => {
                let row = sqlx::query("SELECT name FROM users WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;
                let name: String = row
                    .ok_or_else(|| Error::InvalidInput("redeeming user not found".into()))?
                    .try_get("name")?;
                (id, name, None)
            }
            RedeemIdentity::NewGuest { name } => {
                let new_id = Uuid::new_v4();
                let (raw_token, user_sha256) = generate_token();
                sqlx::query(
                    "INSERT INTO users \
                       (id, name, token_sha256, is_admin, scoped_session_id, \
                        created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(new_id.to_string())
                .bind(name)
                .bind(&user_sha256)
                .bind(false)
                .bind(&invite.session_id)
                .bind(&now_str)
                .bind(&now_str)
                .execute(&mut *tx)
                .await?;
                (new_id, name.to_owned(), Some(raw_token))
            }
        };

        // Same upsert semantics as `upsert_participant` — idempotent
        // on a re-redeem, clears `left_at` if the caller previously
        // walked out of the session.
        sqlx::query(
            "INSERT INTO participants (session_id, user_id, role, joined_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT (session_id, user_id) \
             DO UPDATE SET role = excluded.role, left_at = NULL",
        )
        .bind(&invite.session_id)
        .bind(user_id.to_string())
        .bind(invite.role.as_str())
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(RedeemOutcome {
            invite,
            user_id,
            user_name,
            issued_token,
            was_already_member: false,
        })
    }

    async fn insert_audit_event(&self, event: &AuditEvent) -> Result<i64> {
        // `Value::Null` is stored as SQL NULL (not the literal string
        // `"null"`) so queries that filter on a present detail work
        // naturally — `detail IS NOT NULL` means "has extra data".
        let detail_json = if event.detail.is_null() {
            None
        } else {
            Some(
                serde_json::to_string(&event.detail)
                    .map_err(|e| Error::InvalidInput(format!("invalid detail: {e}")))?,
            )
        };
        let result = sqlx::query(
            "INSERT INTO audit_events \
               (ts, actor_id, actor_name, event_type, session_id, detail) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(rfc3339(event.ts))
        .bind(event.actor_id.map(|u| u.to_string()))
        .bind(event.actor_name.as_ref())
        .bind(event.event_type.as_str())
        .bind(event.session_id.as_ref())
        .bind(detail_json)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    async fn list_audit_events(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        // Dynamic WHERE — same safety pattern as
        // `list_sessions_for_user`: every `?` is bound through sqlx,
        // the only strings that reach the SQL text are the hardcoded
        // `IN (?, ?, ...)` placeholders computed from the length of
        // `event_types`. No caller-supplied string is ever
        // interpolated.
        let mut sql = String::from(
            "SELECT id, ts, actor_id, actor_name, event_type, session_id, detail \
             FROM audit_events WHERE 1 = 1",
        );
        if filter.since.is_some() {
            sql.push_str(" AND ts >= ?");
        }
        if filter.until.is_some() {
            sql.push_str(" AND ts < ?");
        }
        if filter.actor_id.is_some() {
            sql.push_str(" AND actor_id = ?");
        }
        if filter.session_id.is_some() {
            sql.push_str(" AND session_id = ?");
        }
        if !filter.event_types.is_empty() {
            sql.push_str(" AND event_type IN (");
            for i in 0..filter.event_types.len() {
                if i > 0 {
                    sql.push_str(", ");
                }
                sql.push('?');
            }
            sql.push(')');
        }
        sql.push_str(" ORDER BY ts DESC, id DESC");
        // An unset `limit` falls back to 100 — sane default for the
        // CLI and the session-detail timeline. Callers that want
        // everything pass `Some(i64::MAX)` (or the documented
        // `LIMIT -1` sentinel via a higher layer).
        let limit = filter.limit.unwrap_or(100);
        sql.push_str(" LIMIT ?");
        if filter.offset > 0 {
            sql.push_str(" OFFSET ?");
        }

        let mut q = sqlx::query(&sql);
        if let Some(since) = filter.since {
            q = q.bind(rfc3339(since));
        }
        if let Some(until) = filter.until {
            q = q.bind(rfc3339(until));
        }
        if let Some(actor_id) = filter.actor_id {
            q = q.bind(actor_id.to_string());
        }
        if let Some(session_id) = filter.session_id.as_ref() {
            q = q.bind(session_id.clone());
        }
        for et in &filter.event_types {
            q = q.bind(et.as_str().to_string());
        }
        q = q.bind(limit);
        if filter.offset > 0 {
            q = q.bind(filter.offset);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_audit_event).collect()
    }

    // ── Auth — email registration (v0.1.2 pending-row design) ────────────

    async fn upsert_pending_registration(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
        otp_code: &str,
        otp_expires_at: DateTime<Utc>,
    ) -> Result<()> {
        // Single statement: insert or replace the pending row keyed by
        // email. Reset `otp_failure_count` on overwrite so a re-register
        // from the same address starts a clean 5-strike window — the
        // previous attempt's lockout state is
        // intentionally discarded because re-registration is the
        // user-driven recovery path. The pending row carries no
        // authority of its own (no `users` row, no token), so
        // overwriting it cannot impersonate or hijack anyone.
        let now_str = now_rfc3339();
        sqlx::query(
            "INSERT INTO pending_registrations \
                 (email, display_name, password_hash, otp_code, otp_expires_at, \
                  otp_failure_count, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 0, ?, ?) \
             ON CONFLICT(email) DO UPDATE SET \
                 display_name      = excluded.display_name, \
                 password_hash     = excluded.password_hash, \
                 otp_code          = excluded.otp_code, \
                 otp_expires_at    = excluded.otp_expires_at, \
                 otp_failure_count = 0, \
                 updated_at        = excluded.updated_at",
        )
        .bind(email)
        .bind(display_name)
        .bind(password_hash)
        .bind(otp_code)
        .bind(rfc3339(otp_expires_at))
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn latest_pending_registration_at(&self, email: &str) -> Result<Option<DateTime<Utc>>> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT updated_at FROM pending_registrations WHERE email = ?")
                .bind(email)
                .fetch_optional(&self.pool)
                .await?;
        parse_optional_datetime(row, "pending_registrations.updated_at")
    }

    async fn delete_pending_registration(&self, email: &str, otp_code: &str) -> Result<()> {
        sqlx::query("DELETE FROM pending_registrations WHERE email = ? AND otp_code = ?")
            .bind(email)
            .bind(otp_code)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn verify_pending_registration(
        &self,
        email: &str,
        code: &str,
    ) -> Result<PendingVerifyResult> {
        use PendingVerifyResult::*;

        // Atomic compare-and-consume on the pending row. SQLite
        // serialises writes per database so either the `WHERE` guard
        // matches (we win the row) or it doesn't (we lose and the
        // second write sees the updated state). The same CAS shape
        // the old `verify_otp` used, but now scoped to a single
        // pending row keyed by email.

        let now = Utc::now();
        let now_str = rfc3339(now);

        // Run inside a transaction so the consume + materialize
        // happens atomically. A crash between the two halves would
        // either leave a verified `users` row with no auth path
        // (the pending row was already gone) or a stale pending row
        // with a real `users` shadow (next verify would conflict on
        // the unique email index).
        //
        // Importantly, the FIRST statement in this transaction is
        // always a write — either the success-path DELETE-RETURNING
        // or the failure-path UPDATE bump below. SQLite acquires
        // RESERVED on the very first write, which means a SELECT
        // can never escalate from SHARED → RESERVED mid-transaction
        // (the canonical SQLite write-write deadlock pattern). With
        // `busy_timeout = 5s` (set in `new`) the loser of any race
        // simply waits for the winner to commit instead of failing.
        let mut tx = self.pool.begin().await?;

        // Atomic delete-and-return on the success conditions: the
        // single statement either consumes the row (and hands us
        // back the credentials we need to materialize the user) or
        // returns nothing (and we fall through to the failure
        // accounting below).
        let consumed: Option<(String, String)> = sqlx::query_as(
            "DELETE FROM pending_registrations \
             WHERE email = ?1 \
               AND otp_code = ?2 \
               AND otp_expires_at > ?3 \
               AND otp_failure_count < 5 \
             RETURNING display_name, password_hash",
        )
        .bind(email)
        .bind(code)
        .bind(&now_str)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((display_name, password_hash)) = consumed {
            // Happy path. The pending row is already gone; finish
            // up by inserting the user inside the same transaction.
            // The user is materialized with `verified = TRUE`
            // (because the OTP just proved mailbox ownership) and
            // `session_enabled = FALSE` (because self-served signups
            // must be approved by an admin before they can spawn or
            // attach to sessions — the critical adversarial finding
            // fix).
            let id = Uuid::new_v4();
            let (raw, sha256_hex) = generate_token();
            sqlx::query(
                "INSERT INTO users \
                   (id, name, token_sha256, is_admin, scoped_session_id, \
                    email, password_hash, verified, session_enabled, \
                    created_at, updated_at) \
                 VALUES (?, ?, ?, FALSE, NULL, ?, ?, TRUE, FALSE, ?, ?)",
            )
            .bind(id.to_string())
            .bind(&display_name)
            .bind(&sha256_hex)
            .bind(email)
            .bind(&password_hash)
            .bind(&now_str)
            .bind(&now_str)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(ref db) = e
                    && db.is_unique_violation()
                {
                    // Display name collision OR (much rarer) the
                    // email is already in `users` from another
                    // path. Either way the right answer is to fail
                    // verification — the user has to retry with a
                    // different display name.
                    return Error::Conflict(format!(
                        "display name '{display_name}' is already taken — \
                         please re-register with a different name"
                    ));
                }
                Error::Storage(e)
            })?;
            tx.commit().await?;

            let user = User {
                id,
                name: display_name,
                is_admin: false,
                scoped_session_id: None,
                email: Some(email.to_string()),
                session_enabled: false,
                created_at: now,
                updated_at: now,
            };
            return Ok(Success {
                user,
                raw_token: raw,
            });
        }

        // No eligible row. Either the email has no pending row at all,
        // the OTP code did not match, the OTP has expired, or the row
        // is already locked. Try to bump the counter on a present-but-
        // not-locked row so wrong codes still feed the 5-strike
        // lockout.
        // The bump is gated on `otp_expires_at > now` so wrong codes
        // against an *expired* row do NOT burn a strike — letting them
        // would give a stuffing attacker free access to the lockout
        // budget by hammering long-dead pending rows. The auth service
        // surfaces "expired" and "wrong code" identically to the
        // caller, so collapsing the lockout-counting path here doesn't
        // leak anything observable.
        let bumped: Option<i64> = sqlx::query_scalar(
            "UPDATE pending_registrations \
             SET otp_failure_count = otp_failure_count + 1 \
             WHERE email = ?1 \
               AND otp_failure_count < 5 \
               AND otp_expires_at > ?2 \
             RETURNING otp_failure_count",
        )
        .bind(email)
        .bind(&now_str)
        .fetch_optional(&mut *tx)
        .await?;

        tx.commit().await?;

        match bumped {
            Some(new_count) if new_count >= 5 => Ok(Locked),
            Some(new_count) => Ok(Failure {
                remaining: (5 - new_count) as u32,
            }),
            None => {
                // No row at all, or the row is already locked. We
                // collapse both into Expired/Locked depending on the
                // row's actual state — but the auth service surfaces
                // the same generic error to the caller either way to
                // avoid enumerating which addresses are pending.
                let locked: Option<i64> = sqlx::query_scalar(
                    "SELECT otp_failure_count FROM pending_registrations \
                     WHERE email = ?",
                )
                .bind(email)
                .fetch_optional(&self.pool)
                .await?;
                match locked {
                    Some(fc) if fc >= 5 => Ok(Locked),
                    _ => Ok(Expired),
                }
            }
        }
    }

    async fn sweep_pending_registrations(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query("DELETE FROM pending_registrations WHERE updated_at < ?")
            .bind(rfc3339(cutoff))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM users WHERE email = ?"))
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| row_to_user(&r)).transpose()
    }

    // ── Admin user management ─────────────────────────────────────────────

    async fn list_accounts(&self) -> Result<Vec<User>> {
        let rows = sqlx::query(&format!(
            "SELECT {USER_COLS} FROM users \
             WHERE scoped_session_id IS NULL \
             ORDER BY created_at DESC"
        ))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_user).collect()
    }

    async fn list_accounts_filtered(&self, filter: &AccountFilter) -> Result<(Vec<User>, i64)> {
        // Build the dynamic WHERE clause. Every branch appends to
        // `conditions` and pushes bind values onto `binds` in order so
        // the `?` placeholders line up with `.bind()` calls.
        let mut conditions = vec!["scoped_session_id IS NULL".to_owned()];
        let mut binds: Vec<String> = Vec::new();

        if let Some(ref q) = filter.query {
            let like = format!("%{q}%");
            conditions.push(
                "(name LIKE ? COLLATE NOCASE OR email LIKE ? COLLATE NOCASE)".to_owned(),
            );
            binds.push(like.clone());
            binds.push(like);
        }

        if let Some(status) = filter.status {
            match status {
                AccountStatus::Enabled => {
                    conditions.push("session_enabled = TRUE AND verified = TRUE".to_owned());
                }
                AccountStatus::Disabled => {
                    conditions.push("session_enabled = FALSE AND verified = TRUE".to_owned());
                }
                AccountStatus::Pending => {
                    conditions.push("verified = FALSE".to_owned());
                }
            }
        }

        let where_clause = conditions.join(" AND ");

        let count_sql = format!("SELECT COUNT(*) AS cnt FROM users WHERE {where_clause}");
        let mut count_query = sqlx::query(&count_sql);
        for v in &binds {
            count_query = count_query.bind(v);
        }
        let total: i64 = count_query.fetch_one(&self.pool).await?.get("cnt");

        let data_sql = format!(
            "SELECT {USER_COLS} FROM users WHERE {where_clause} \
             ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        let mut data_query = sqlx::query(&data_sql);
        for v in &binds {
            data_query = data_query.bind(v);
        }
        data_query = data_query.bind(filter.limit).bind(filter.offset);

        let rows = data_query.fetch_all(&self.pool).await?;
        let users: Vec<User> = rows.iter().map(row_to_user).collect::<Result<_>>()?;

        Ok((users, total))
    }

    async fn find_user_by_id(&self, user_id: Uuid) -> Result<Option<User>> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM users WHERE id = ?"))
            .bind(user_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| row_to_user(&r)).transpose()
    }

    async fn set_session_enabled(&self, user_id: Uuid, enabled: bool) -> Result<User> {
        let now_str = now_rfc3339();
        let row = sqlx::query(&format!(
            "UPDATE users SET session_enabled = ?, updated_at = ? \
             WHERE id = ? \
             RETURNING {USER_COLS}"
        ))
        .bind(enabled)
        .bind(&now_str)
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let row = row.ok_or_else(|| Error::InvalidInput(format!("user {user_id} not found")))?;
        row_to_user(&row)
    }

    async fn refresh_user_token(&self, user_id: Uuid) -> Result<String> {
        let (raw, hash) = generate_token();
        sqlx::query("UPDATE users SET token_sha256 = ?, updated_at = ? WHERE id = ?")
            .bind(&hash)
            .bind(now_rfc3339())
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(raw)
    }

    async fn check_login_lockout(&self, user_id: Uuid) -> Result<Option<DateTime<Utc>>> {
        // Read the row's stored lockout. Three states matter:
        //   1. NULL: idle row, no lockout — return None.
        //   2. > now: live lockout — return Some(time).
        //   3. <= now: window has elapsed — lazily clear `login_failed_count`
        //      and `login_locked_until` so the next failure starts a fresh
        //      5-strike window instead of immediately re-locking on a
        //      stale counter, then return None.
        //
        // The clear is the only write on this read path; concurrent
        // record_login_failure calls during a clear-and-retry race resolve
        // consistently because both branches in record_login_failure's CASE
        // statement handle "no live lock" identically.
        // The outer Option is "row exists"; the inner Option<String>
        // is the nullable column value. Without the inner Option,
        // sqlx's String decoder maps SQL NULL to an empty string,
        // which then trips parse_optional_datetime with "premature
        // end of input".
        let raw: Option<Option<String>> = sqlx::query_scalar::<_, Option<String>>(
            "SELECT login_locked_until FROM users WHERE id = ?",
        )
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let stored = parse_optional_datetime(raw.flatten(), "users.login_locked_until")?;
        let Some(until) = stored else {
            return Ok(None);
        };
        let now = Utc::now();
        if until > now {
            return Ok(Some(until));
        }
        sqlx::query(
            "UPDATE users SET login_failed_count = 0, login_locked_until = NULL WHERE id = ?",
        )
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(None)
    }

    async fn record_login_failure(
        &self,
        user_id: Uuid,
        lockout_duration: chrono::Duration,
    ) -> Result<LoginFailureOutcome> {
        // Atomic CAS that handles three branches in one UPDATE:
        //
        // 1. Live lock (`login_locked_until > now`): leave count and
        //    lockout untouched. Returning the existing window prevents
        //    a hammering attacker from sliding the unlock time forward
        //    indefinitely — the test
        //    `record_login_failure_while_locked_keeps_existing_lock`
        //    pins this invariant.
        // 2. Stale lock (`login_locked_until <= now`): the previous
        //    window has expired; reset the counter to 1 and clear the
        //    lock so this failure starts a fresh 5-strike window.
        //    Defensive — `check_login_lockout` should normally clear
        //    first, but a racing call may not have run yet.
        // 3. No lock: increment the counter; if the post-bump count
        //    crosses the 5-strike threshold, stamp `login_locked_until`
        //    to `now + lockout_duration`.
        //
        // The literal `5` matches the OTP failure threshold so both
        // throttles agree at a glance. Lockout duration is supplied by
        // the caller (`AuthService`) so the storage layer stays free
        // of policy constants.
        let now = Utc::now();
        let now_str = rfc3339(now);
        let lock_until_str = rfc3339(now + lockout_duration);
        let row: Option<(i64, Option<String>)> = sqlx::query_as(
            "UPDATE users \
             SET login_failed_count = CASE \
                     WHEN login_locked_until IS NOT NULL AND login_locked_until > ?1 \
                         THEN login_failed_count \
                     WHEN login_locked_until IS NOT NULL AND login_locked_until <= ?1 \
                         THEN 1 \
                     ELSE login_failed_count + 1 \
                 END, \
                 login_locked_until = CASE \
                     WHEN login_locked_until IS NOT NULL AND login_locked_until > ?1 \
                         THEN login_locked_until \
                     WHEN login_locked_until IS NOT NULL AND login_locked_until <= ?1 \
                         THEN NULL \
                     WHEN login_failed_count + 1 >= 5 \
                         THEN ?2 \
                     ELSE NULL \
                 END \
             WHERE id = ?3 \
             RETURNING login_failed_count, login_locked_until",
        )
        .bind(&now_str)
        .bind(&lock_until_str)
        .bind(user_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let (count, locked_raw) =
            row.ok_or_else(|| Error::Auth(format!("user {user_id} not found")))?;
        let locked_until = parse_optional_datetime(locked_raw, "users.login_locked_until")?;
        if let Some(until) = locked_until
            && until > now
        {
            return Ok(LoginFailureOutcome::Locked { until });
        }
        let remaining = 5_i64.saturating_sub(count).max(0) as u32;
        Ok(LoginFailureOutcome::Recorded { remaining })
    }

    async fn clear_login_failures(&self, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE users SET login_failed_count = 0, login_locked_until = NULL WHERE id = ?",
        )
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── User-owned targets ────────────────────────────────────────────────

    async fn create_user_target(&self, params: CreateUserTargetParams) -> Result<UserTarget> {
        let id = nanoid::nanoid!(21);
        let now = Utc::now();
        let now_str = rfc3339(now);
        let args_json = serde_json::to_string(&params.args)?;
        let env_json = serde_json::to_string(&params.env)?;
        let tags_json = serde_json::to_string(&params.tags)?;

        sqlx::query(
            "INSERT INTO user_targets \
             (id, user_id, name, display, command, args, env, tags, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(params.user_id.to_string())
        .bind(&params.name)
        .bind(&params.display)
        .bind(&params.command)
        .bind(&args_json)
        .bind(&env_json)
        .bind(&tags_json)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db) = e
                && db.is_unique_violation()
            {
                return Error::Conflict(format!("target name '{}' already exists", params.name));
            }
            Error::Storage(e)
        })?;

        Ok(UserTarget {
            id,
            user_id: params.user_id,
            name: params.name,
            display: params.display,
            command: params.command,
            args: params.args,
            env: params.env,
            tags: params.tags,
            created_at: now,
            updated_at: now,
        })
    }

    async fn list_user_targets(&self, user_id: Uuid) -> Result<Vec<UserTarget>> {
        let rows = sqlx::query(
            "SELECT id, user_id, name, display, command, args, env, tags, created_at, updated_at \
             FROM user_targets WHERE user_id = ? ORDER BY name",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_user_target).collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_user_target(
        &self,
        id: &str,
        user_id: Uuid,
        display: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        tags: &[String],
    ) -> Result<UserTarget> {
        let args_json = serde_json::to_string(args)?;
        let env_json = serde_json::to_string(env)?;
        let tags_json = serde_json::to_string(tags)?;
        let now_str = now_rfc3339();

        // CAS guard: refuse to edit a target while any **active**
        // session still references it. The old code only checked
        // `(id, user_id)` — if the owner edited a target mid-session,
        // the next PTY attach (global `TargetEngine::resolve` miss,
        // then `user_targets` re-read) would silently pick up the
        // new `command`/`args`/`env`, so the running session's
        // identity drifted out from under it. The `NOT EXISTS`
        // subquery lives inside the same UPDATE statement, so SQLite
        // evaluates it against the same write-lock snapshot as the
        // row match — no TOCTOU window between a separate probe and
        // the update. Closed sessions don't block edits: they never
        // re-attach, so the drift risk does not apply to them.
        let result = sqlx::query(
            "UPDATE user_targets \
             SET display = ?, command = ?, args = ?, env = ?, tags = ?, updated_at = ? \
             WHERE id = ? AND user_id = ? \
               AND NOT EXISTS ( \
                   SELECT 1 FROM sessions \
                   WHERE sessions.user_target_id = user_targets.id \
                     AND sessions.status = 'active' \
               )",
        )
        .bind(display)
        .bind(command)
        .bind(&args_json)
        .bind(&env_json)
        .bind(&tags_json)
        .bind(&now_str)
        .bind(id)
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(self
                .classify_user_target_mutation_failure(id, user_id)
                .await);
        }

        self.find_user_target_by_id(id)
            .await?
            .ok_or_else(|| Error::Internal("target disappeared after update".into()))
    }

    async fn delete_user_target(&self, id: &str, user_id: Uuid) -> Result<()> {
        // Same CAS guard as `update_user_target`. Deleting a target
        // that a live session depends on would turn that session into
        // an orphan the next time the WS handler tried to resolve it,
        // because the `user_targets` row backing the attach config is
        // gone. Owners who really want to rebuild a target must close
        // their session first — the error classifier below tells them
        // exactly that.
        let result = sqlx::query(
            "DELETE FROM user_targets \
             WHERE id = ? AND user_id = ? \
               AND NOT EXISTS ( \
                   SELECT 1 FROM sessions \
                   WHERE sessions.user_target_id = user_targets.id \
                     AND sessions.status = 'active' \
               )",
        )
        .bind(id)
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(self
                .classify_user_target_mutation_failure(id, user_id)
                .await);
        }
        Ok(())
    }

    async fn find_user_target_by_id(&self, id: &str) -> Result<Option<UserTarget>> {
        let row = sqlx::query(
            "SELECT id, user_id, name, display, command, args, env, tags, created_at, updated_at \
             FROM user_targets WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| row_to_user_target(&r)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::InputMode;

    /// Read the raw `left_at` string for a (session_id, user_id)
    /// participant row, bypassing the `left_at IS NULL` filter that
    /// the public `list_participants` API applies. Tests need this to
    /// assert that a boot-time sweep did NOT overwrite a participant's
    /// historical `left_at` — the public API hides `left_at` once
    /// it's set, so it can't distinguish "untouched" from "rewritten
    /// to a new timestamp".
    async fn raw_left_at(pool: &Pool<Sqlite>, session_id: &str, user_id: Uuid) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT left_at FROM participants WHERE session_id = ? AND user_id = ?",
        )
        .bind(session_id)
        .bind(user_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Regression: `close_stale_sessions` must not rewrite the
    /// `left_at` of a participant whose session was already closed
    /// before the sweep ran. The pre-fix implementation located
    /// "sessions we just closed" via
    /// `WHERE status='closed' AND closed_at = ?`, which conflates
    /// "closed in this sweep" with "historically closed at a
    /// colliding timestamp string". Under literal string collision
    /// the buggy version rewrote the historical participant's
    /// `left_at` to the sweep's `now`; the RETURNING-based fix keys
    /// the second UPDATE off the exact row ids it just touched, so
    /// no secondary attribute can misidentify a historical row.
    ///
    /// This test cannot force the exact nanosecond collision without
    /// mocking the wall-clock source, so instead it pins the
    /// structural invariant directly: after the sweep, the
    /// historical participant row is bit-for-bit unchanged. Any
    /// future re-introduction of a time-equality heuristic would
    /// still be caught by a code-review pass; this test locks down
    /// the post-sweep data shape under the common path.
    #[tokio::test]
    async fn close_stale_sessions_only_touches_just_closed_participants() {
        let store = SqliteStorage::new_memory().await.unwrap();
        let (alice, _) = store.create_user("alice", false).await.unwrap();

        // Historical closed session with a deterministic `left_at`
        // stamp. We overwrite `closed_at` and `left_at` to a fixed
        // value so the assertion below is exact-string equality, not
        // a fuzzy "some timestamp near now" check.
        let s_old = store
            .create_session_with_owner(alice.id, "shell", InputMode::Serialized, None)
            .await
            .unwrap();
        store
            .close_session(&s_old.id, CloseReason::Owner)
            .await
            .unwrap();
        let stamped = "2099-01-01T00:00:00+00:00".to_string();
        sqlx::query("UPDATE sessions SET closed_at = ? WHERE id = ?")
            .bind(&stamped)
            .bind(&s_old.id)
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE participants SET left_at = ? WHERE session_id = ? AND user_id = ?")
            .bind(&stamped)
            .bind(&s_old.id)
            .bind(alice.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();

        // New active session that the sweep should actually close.
        let s_new = store
            .create_session_with_owner(alice.id, "shell", InputMode::Serialized, None)
            .await
            .unwrap();

        let closed = store
            .close_stale_sessions(CloseReason::Startup)
            .await
            .unwrap();
        assert_eq!(closed, 1, "only the one active session should be swept");

        // The historical row is untouched: same closed_at, same
        // left_at, both still the `stamped` string.
        let after_old_left = raw_left_at(&store.pool, &s_old.id, alice.id).await;
        assert_eq!(
            after_old_left.as_deref(),
            Some(stamped.as_str()),
            "sweep must not rewrite left_at on sessions it did not itself close"
        );

        // Sanity: the new session's participant got its `left_at`
        // stamped by this sweep (proves the second UPDATE still
        // runs under the RETURNING-driven id set).
        let after_new_left = raw_left_at(&store.pool, &s_new.id, alice.id).await;
        assert!(
            after_new_left.is_some(),
            "sweep must still settle the participants it did just close"
        );
    }

    /// Pins the atomic guarantee of
    /// [`Storage::find_active_participant_role`]: when the session is
    /// closed, the lookup returns `None` for a former participant,
    /// EVEN THOUGH the participant row still exists (the history
    /// view reads it). Previously the invite-redeem short path
    /// reassembled the same check out of two independent queries
    /// (`get_session` + `list_participants`), which could disagree
    /// across a concurrent close. Routing the short path through
    /// this method makes the check one statement, so the closed-row
    /// case cannot leak back as "Some(role)".
    #[tokio::test]
    async fn find_active_participant_role_returns_none_for_closed_session() {
        let store = SqliteStorage::new_memory().await.unwrap();
        let (alice, _) = store.create_user("alice", false).await.unwrap();
        let session = store
            .create_session_with_owner(alice.id, "shell", InputMode::Serialized, None)
            .await
            .unwrap();

        // Sanity: active session returns the owner's role.
        let active = store
            .find_active_participant_role(&session.id, alice.id)
            .await
            .unwrap();
        assert_eq!(
            active,
            Some(Role::Owner),
            "active-session owner lookup must return Some(Owner)"
        );

        // Close the session — this flips `status` to 'closed' and
        // stamps `left_at` on every participant in one transaction.
        store
            .close_session(&session.id, CloseReason::Owner)
            .await
            .unwrap();

        // After close, the same lookup must return None. Either
        // predicate (`left_at IS NULL` or `s.status = 'active'`)
        // alone is enough to filter it out; the test runs through
        // the JOIN so both arms are exercised.
        let after = store
            .find_active_participant_role(&session.id, alice.id)
            .await
            .unwrap();
        assert_eq!(
            after, None,
            "closed-session participant lookup must return None"
        );
    }

    /// Companion case: a stranger must never see `Some(role)` for a
    /// session they never joined, regardless of status. Locks down
    /// the "user_id = ?" predicate so a future refactor that drops
    /// it (e.g. collapsing to `WHERE session_id = ?`) would be
    /// caught.
    #[tokio::test]
    async fn find_active_participant_role_returns_none_for_non_member() {
        let store = SqliteStorage::new_memory().await.unwrap();
        let (alice, _) = store.create_user("alice", false).await.unwrap();
        let (bob, _) = store.create_user("bob", false).await.unwrap();
        let session = store
            .create_session_with_owner(alice.id, "shell", InputMode::Serialized, None)
            .await
            .unwrap();

        let got = store
            .find_active_participant_role(&session.id, bob.id)
            .await
            .unwrap();
        assert_eq!(got, None, "non-member lookup must return None");
    }

    #[tokio::test]
    async fn pending_registration_upsert_then_verify_creates_user() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let expires = Utc::now() + chrono::Duration::minutes(15);

        s.upsert_pending_registration(
            "alice@example.com",
            "alice",
            "hash_placeholder",
            "123456",
            expires,
        )
        .await
        .unwrap();

        // No `users` row exists yet — the pending state lives in
        // its own table and carries no authority.
        assert!(
            s.get_user_by_email("alice@example.com")
                .await
                .unwrap()
                .is_none()
        );

        // Verify with the right code. The pending row is consumed
        // and a fresh user materializes in the same transaction.
        let result = s
            .verify_pending_registration("alice@example.com", "123456")
            .await
            .unwrap();
        let (user, token) = match result {
            PendingVerifyResult::Success { user, raw_token } => (user, raw_token),
            other => panic!("expected Success, got {other:?}"),
        };
        assert_eq!(user.email.as_deref(), Some("alice@example.com"));
        assert_eq!(user.name, "alice");
        // Critical: a self-served signup is inert until an admin
        // approves it. Without this gate, anyone with email +
        // SMTP enabled could spawn a shell on the gateway host.
        assert!(
            !user.session_enabled,
            "self-registered accounts must start with session_enabled = FALSE"
        );

        // The minted token validates back to the same user.
        let authed = s.validate_token(&token).await.unwrap();
        assert_eq!(authed.id, user.id);

        // The pending row is gone, so a second verify is Expired.
        let second = s
            .verify_pending_registration("alice@example.com", "123456")
            .await
            .unwrap();
        assert!(matches!(second, PendingVerifyResult::Expired));
    }

    #[tokio::test]
    async fn pending_registration_upsert_overwrites_in_place() {
        // Re-registering the same address must overwrite the previous
        // pending row in place AND reset the failure counter so the
        // user-driven retry is not stuck behind a stale lockout. The
        // pending row carries no authority so this is safe — it's
        // the takeover-fix invariant from the v0.1.2 finding.
        let s = SqliteStorage::new_memory().await.unwrap();
        let expires = Utc::now() + chrono::Duration::minutes(15);
        s.upsert_pending_registration("retry@example.com", "alice", "old_hash", "111111", expires)
            .await
            .unwrap();

        // Burn 3 wrong attempts on the original row.
        for _ in 0..3 {
            let _ = s
                .verify_pending_registration("retry@example.com", "000000")
                .await
                .unwrap();
        }

        // Re-register with a different password and display name —
        // the row is overwritten and the failure counter resets.
        s.upsert_pending_registration("retry@example.com", "alicia", "new_hash", "222222", expires)
            .await
            .unwrap();

        // The old code is now wrong (overwritten) and counts as one
        // bump; remaining must be 4 (not 1, which would imply the
        // old counter survived).
        let r = s
            .verify_pending_registration("retry@example.com", "111111")
            .await
            .unwrap();
        assert!(
            matches!(r, PendingVerifyResult::Failure { remaining: 4 }),
            "expected fresh 5-strike window after re-register, got {r:?}"
        );
    }

    #[tokio::test]
    async fn verify_pending_registration_failure_sequence_locks_at_five() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let expires = Utc::now() + chrono::Duration::minutes(15);
        s.upsert_pending_registration("bob@example.com", "bob", "h", "999999", expires)
            .await
            .unwrap();

        for i in 0..5u32 {
            let r = s
                .verify_pending_registration("bob@example.com", "000000")
                .await
                .unwrap();
            if i < 4 {
                assert!(
                    matches!(r, PendingVerifyResult::Failure { remaining } if remaining == 4 - i)
                );
            } else {
                assert!(matches!(r, PendingVerifyResult::Locked));
            }
        }
        // After lockout, even the correct code must not succeed.
        assert!(matches!(
            s.verify_pending_registration("bob@example.com", "999999")
                .await
                .unwrap(),
            PendingVerifyResult::Locked,
        ));
    }

    #[tokio::test]
    async fn verify_pending_registration_unknown_email_collapses_to_expired() {
        // Unknown addresses must look identical to expired pending
        // rows from the storage layer's perspective — the auth
        // service then maps both to a single generic error so an
        // unauthenticated caller cannot enumerate which addresses
        // have started a registration.
        let s = SqliteStorage::new_memory().await.unwrap();
        let r = s
            .verify_pending_registration("ghost@example.com", "123456")
            .await
            .unwrap();
        assert!(matches!(r, PendingVerifyResult::Expired));
    }

    #[tokio::test]
    async fn delete_pending_registration_clears_rate_limit() {
        // The SMTP-failure rollback primitive: the pending row is
        // removed so the user is not stuck behind the 60-second
        // rate limit on a code that was never delivered.
        let s = SqliteStorage::new_memory().await.unwrap();
        let expires = Utc::now() + chrono::Duration::minutes(15);
        s.upsert_pending_registration("rb@example.com", "rb", "h", "111111", expires)
            .await
            .unwrap();
        assert!(
            s.latest_pending_registration_at("rb@example.com")
                .await
                .unwrap()
                .is_some()
        );

        // Matching OTP deletes the row.
        s.delete_pending_registration("rb@example.com", "111111")
            .await
            .unwrap();

        assert!(
            s.latest_pending_registration_at("rb@example.com")
                .await
                .unwrap()
                .is_none(),
            "matching OTP must delete the pending row"
        );
    }

    #[tokio::test]
    async fn delete_pending_registration_skips_mismatched_otp() {
        // A concurrent registration may have overwritten the row with a
        // new OTP. The rollback must not delete the newer row.
        let s = SqliteStorage::new_memory().await.unwrap();
        let expires = Utc::now() + chrono::Duration::minutes(15);
        s.upsert_pending_registration("race@example.com", "r", "h", "111111", expires)
            .await
            .unwrap();
        // Simulate concurrent overwrite with a new OTP.
        s.upsert_pending_registration("race@example.com", "r", "h", "222222", expires)
            .await
            .unwrap();

        // Rollback from the first request uses the stale OTP.
        s.delete_pending_registration("race@example.com", "111111")
            .await
            .unwrap();

        // The row with the newer OTP must survive.
        assert!(
            s.latest_pending_registration_at("race@example.com")
                .await
                .unwrap()
                .is_some(),
            "mismatched OTP must not delete the concurrent registration's row"
        );
    }

    #[tokio::test]
    async fn refresh_user_token_rotates_token() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let (user, _) = s.create_user("carol", false).await.unwrap();
        let tok2 = s.refresh_user_token(user.id).await.unwrap();
        let authed = s.validate_token(&tok2).await.unwrap();
        assert_eq!(authed.id, user.id);
    }

    #[tokio::test]
    async fn change_password_and_rotate_token_is_atomic() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let (user, _) = s.create_user("atomic", false).await.unwrap();
        let old_token = s.refresh_user_token(user.id).await.unwrap();

        let new_token = s
            .change_password_and_rotate_token(user.id, "new-hash-value")
            .await
            .unwrap();

        // New token is valid.
        assert_ne!(new_token, old_token);
        let authed = s.validate_token(&new_token).await.unwrap();
        assert_eq!(authed.id, user.id);

        // Old token is invalidated.
        assert!(s.validate_token(&old_token).await.is_err());

        // Password hash was updated.
        let hash = s.get_password_hash(user.id).await.unwrap();
        assert_eq!(hash.as_deref(), Some("new-hash-value"));
    }

    #[tokio::test]
    async fn schema_has_pending_registrations_and_session_enabled() {
        let s = SqliteStorage::new_memory().await.unwrap();
        // Pending-registration table is real and selectable.
        sqlx::query("SELECT email, otp_failure_count FROM pending_registrations LIMIT 1")
            .execute(&s.pool)
            .await
            .unwrap();
        // session_enabled column is present on users.
        sqlx::query("SELECT session_enabled FROM users LIMIT 1")
            .execute(&s.pool)
            .await
            .unwrap();
        // Legacy email_verifications table was dropped.
        let err = sqlx::query("SELECT 1 FROM email_verifications")
            .execute(&s.pool)
            .await;
        assert!(
            err.is_err(),
            "email_verifications must be dropped after migration"
        );
        // user_targets is still here.
        let (user, _) = s.create_user("schematest", false).await.unwrap();
        sqlx::query(
            "INSERT INTO user_targets (id, user_id, name, display, command, created_at, updated_at)
             VALUES ('t1', ?, 'test', 'Test', 'bash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(user.id.to_string())
        .execute(&s.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_accounts_excludes_scoped_guests() {
        // Admin-page listings must not surface invite-minted scoped
        // guests — they have no admin-actionable state and are tied
        // to a single session. The trait method enforces this so
        // every caller gets the same shape.
        let s = SqliteStorage::new_memory().await.unwrap();
        let (alice, _) = s.create_user("alice", false).await.unwrap();
        let session = s
            .create_session_with_owner(alice.id, "shell", InputMode::Serialized, None)
            .await
            .unwrap();
        let (_guest, _) = s.create_scoped_guest("guest1", &session.id).await.unwrap();

        let listed = s.list_accounts().await.unwrap();
        assert!(listed.iter().any(|u| u.id == alice.id));
        assert!(
            listed.iter().all(|u| u.scoped_session_id.is_none()),
            "scoped guests must not appear in list_accounts"
        );
    }

    #[tokio::test]
    async fn list_accounts_filtered_by_status_and_query() {
        let s = SqliteStorage::new_memory().await.unwrap();

        // create_user sets verified=TRUE and session_enabled=TRUE in the DB,
        // so all three users start as "enabled" in AccountStatus terms.
        let (_admin, _) = s.create_user("admin", true).await.unwrap();
        let (alice, _) = s.create_user("alice", false).await.unwrap();
        let (bob, _) = s.create_user("bob", false).await.unwrap();

        // Disable alice → session_enabled=FALSE, verified still TRUE → "Disabled"
        s.set_session_enabled(alice.id, false).await.unwrap();

        // Make bob "pending" by flipping verified to FALSE directly in the DB.
        // There is no trait method for this; create_user always sets verified=TRUE.
        sqlx::query("UPDATE users SET verified = FALSE WHERE id = ?")
            .bind(bob.id.to_string())
            .execute(&s.pool)
            .await
            .unwrap();

        // No filter → all 3 (admin=enabled, alice=disabled, bob=pending)
        let filter = AccountFilter { query: None, status: None, limit: 50, offset: 0 };
        let (rows, total) = s.list_accounts_filtered(&filter).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(rows.len(), 3);

        // Filter by query "ali" → alice only
        let filter = AccountFilter { query: Some("ali".into()), status: None, limit: 50, offset: 0 };
        let (rows, total) = s.list_accounts_filtered(&filter).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].name, "alice");

        // Filter by status Enabled → admin only (session_enabled=TRUE AND verified=TRUE)
        let filter = AccountFilter {
            query: None, status: Some(AccountStatus::Enabled), limit: 50, offset: 0,
        };
        let (rows, total) = s.list_accounts_filtered(&filter).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].name, "admin");

        // Filter by status Disabled → alice (session_enabled=FALSE AND verified=TRUE)
        let filter = AccountFilter {
            query: None, status: Some(AccountStatus::Disabled), limit: 50, offset: 0,
        };
        let (rows, total) = s.list_accounts_filtered(&filter).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].name, "alice");

        // Filter by status Pending → bob (verified=FALSE)
        let filter = AccountFilter {
            query: None, status: Some(AccountStatus::Pending), limit: 50, offset: 0,
        };
        let (rows, total) = s.list_accounts_filtered(&filter).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].name, "bob");

        // Pagination: limit 1, offset 1 — total unaffected
        let filter = AccountFilter { query: None, status: None, limit: 1, offset: 1 };
        let (rows, total) = s.list_accounts_filtered(&filter).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn set_session_enabled_flips_bit_and_returns_updated_user() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let expires = Utc::now() + chrono::Duration::minutes(15);
        s.upsert_pending_registration("u@example.com", "u", "h", "123456", expires)
            .await
            .unwrap();
        let user = match s
            .verify_pending_registration("u@example.com", "123456")
            .await
            .unwrap()
        {
            PendingVerifyResult::Success { user, .. } => user,
            other => panic!("expected Success, got {other:?}"),
        };
        assert!(!user.session_enabled);

        let updated = s.set_session_enabled(user.id, true).await.unwrap();
        assert!(updated.session_enabled);

        // Round-trip via find_user_by_id.
        let read_back = s.find_user_by_id(user.id).await.unwrap().unwrap();
        assert!(read_back.session_enabled);
    }

    // ── Login throttling (Fix #3) ────────────────────────────────────────
    //
    // The throttle primitives operate on `users.id` directly and don't
    // care how the row was created — `create_user` is the simplest path
    // that gives us a real account row to point them at.

    async fn seed_verified_user(s: &SqliteStorage, name: &str) -> Uuid {
        let (u, _) = s.create_user(name, false).await.unwrap();
        u.id
    }

    #[tokio::test]
    async fn login_failure_increments_counter_and_reports_remaining() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let uid = seed_verified_user(&s, "throttle1").await;
        let lock = chrono::Duration::minutes(15);

        // First four wrong attempts: counter climbs, threshold not hit.
        let outcomes = vec![
            s.record_login_failure(uid, lock).await.unwrap(),
            s.record_login_failure(uid, lock).await.unwrap(),
            s.record_login_failure(uid, lock).await.unwrap(),
            s.record_login_failure(uid, lock).await.unwrap(),
        ];
        assert_eq!(
            outcomes,
            vec![
                LoginFailureOutcome::Recorded { remaining: 4 },
                LoginFailureOutcome::Recorded { remaining: 3 },
                LoginFailureOutcome::Recorded { remaining: 2 },
                LoginFailureOutcome::Recorded { remaining: 1 },
            ]
        );

        // Until the threshold is reached the row is not locked, so a
        // probe sees no active lockout.
        assert!(s.check_login_lockout(uid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn login_failure_locks_at_fifth_strike() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let uid = seed_verified_user(&s, "throttle2").await;
        let lock = chrono::Duration::minutes(15);

        // Burn the four "remaining" hops first.
        for _ in 0..4 {
            s.record_login_failure(uid, lock).await.unwrap();
        }
        let before = Utc::now();
        let fifth = s.record_login_failure(uid, lock).await.unwrap();

        match fifth {
            LoginFailureOutcome::Locked { until } => {
                // The lock window is [now+lockout - epsilon, now+lockout + epsilon].
                // We allow generous slack so a slow CI box doesn't flake.
                let expected = before + lock;
                let drift = (until - expected).num_seconds().abs();
                assert!(
                    drift < 5,
                    "lock until {until} drifted {drift}s from {expected}"
                );
            }
            other => panic!("expected Locked, got {other:?}"),
        }

        // The lockout is observable to the read-side check.
        let probe = s.check_login_lockout(uid).await.unwrap();
        assert!(probe.is_some(), "user must register as locked");
    }

    #[tokio::test]
    async fn record_login_failure_while_locked_keeps_existing_lock() {
        // A persistent attacker that keeps hammering during the
        // lockout window must not extend it indefinitely (a
        // self-reinforcing lock would let an attacker permanently
        // keep a real user out by spamming wrong passwords on every
        // attempted unlock). The CAS pins the existing window.
        let s = SqliteStorage::new_memory().await.unwrap();
        let uid = seed_verified_user(&s, "throttle3").await;
        let lock = chrono::Duration::minutes(15);

        for _ in 0..5 {
            s.record_login_failure(uid, lock).await.unwrap();
        }
        let until_first = match s.check_login_lockout(uid).await.unwrap() {
            Some(t) => t,
            None => panic!("expected lockout after 5 strikes"),
        };

        // Slam the door a few more times — the existing window must
        // not get pushed out.
        for _ in 0..3 {
            let r = s.record_login_failure(uid, lock).await.unwrap();
            match r {
                LoginFailureOutcome::Locked { until } => {
                    assert_eq!(
                        until, until_first,
                        "lockout window must not be extended by hammering",
                    );
                }
                other => panic!("expected Locked while locked, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn clear_login_failures_resets_counter_and_lockout() {
        let s = SqliteStorage::new_memory().await.unwrap();
        let uid = seed_verified_user(&s, "throttle4").await;
        let lock = chrono::Duration::minutes(15);

        for _ in 0..5 {
            s.record_login_failure(uid, lock).await.unwrap();
        }
        assert!(s.check_login_lockout(uid).await.unwrap().is_some());

        s.clear_login_failures(uid).await.unwrap();
        assert!(s.check_login_lockout(uid).await.unwrap().is_none());

        // After clear the next failure starts a fresh 5-strike window.
        let r = s.record_login_failure(uid, lock).await.unwrap();
        assert_eq!(r, LoginFailureOutcome::Recorded { remaining: 4 });
    }

    #[tokio::test]
    async fn check_login_lockout_lazily_clears_expired_window() {
        // Once the lockout window has passed in wall time, the next
        // login attempt must see a clean slate — `check_login_lockout`
        // is the chokepoint that turns "stale lock" into "idle row"
        // so the user is not stuck behind a counter that already
        // expired. Negative lockout duration simulates a window that
        // is already in the past at write time, sidestepping the need
        // to actually sleep in tests.
        let s = SqliteStorage::new_memory().await.unwrap();
        let uid = seed_verified_user(&s, "throttle5").await;
        let already_expired = chrono::Duration::seconds(-1);
        for _ in 0..5 {
            s.record_login_failure(uid, already_expired).await.unwrap();
        }

        // The lock is in the past, so check must return None AND
        // reset the counter so the user is not stuck behind a stale
        // lockout the moment they try again.
        let probe = s.check_login_lockout(uid).await.unwrap();
        assert!(probe.is_none(), "expired lockout must read as None");

        let r = s
            .record_login_failure(uid, chrono::Duration::minutes(15))
            .await
            .unwrap();
        assert_eq!(
            r,
            LoginFailureOutcome::Recorded { remaining: 4 },
            "post-clear failure must start a fresh window",
        );
    }
}
