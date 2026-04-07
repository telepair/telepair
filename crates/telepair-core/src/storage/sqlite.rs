use std::str::FromStr;

use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{Pool, Row, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::permission::Role;
use crate::session::{InputMode, InviteToken, Participant, Session, SessionStatus, User};
use crate::storage::Storage;

pub struct SqliteStorage {
    pool: Pool<Sqlite>,
}

impl SqliteStorage {
    pub async fn new(database_url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)?.foreign_keys(true);
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

        // Schema is the single source of truth — pre-1.0, the only
        // supported upgrade path is to delete the DB file and start
        // fresh, so we don't ship compat shims or column-level probes.
        sqlx::raw_sql(include_str!("../../../../migrations/001_initial.sql"))
            .execute(&self.pool)
            .await?;

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

/// RFC3339 timestamp for the current instant — centralizes the format
/// choice so all write paths agree with the parsers above.
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn row_to_user(r: &SqliteRow) -> Result<User> {
    Ok(User {
        id: parse_uuid(r.get("id"))?,
        name: r.get("name"),
        is_admin: r.get("is_admin"),
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
    if let Some(expires_at) = invite.expires_at {
        if expires_at < Utc::now() {
            return Err(Error::InvalidInput("invite token has expired".into()));
        }
    }
    if invite.used_count >= invite.max_uses {
        return Err(Error::InvalidInput("invite token has been fully used".into()));
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
        let row = sqlx::query(
            "SELECT id, name, is_admin, created_at, updated_at FROM users WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| row_to_user(&r)).transpose()
    }
}

impl Storage for SqliteStorage {
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)> {
        let id = Uuid::new_v4();
        let (token, sha256_hex) = generate_token();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        sqlx::query(
            "INSERT INTO users (id, name, token_sha256, is_admin, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(&sha256_hex)
        .bind(is_admin)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await?;

        let user = User {
            id,
            name: name.into(),
            is_admin,
            created_at: now,
            updated_at: now,
        };
        Ok((user, token))
    }

    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>> {
        let row = sqlx::query(
            "SELECT id, name, is_admin, created_at, updated_at FROM users WHERE name = ?",
        )
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
    ) -> Result<Session> {
        let id = nanoid::nanoid!(10);
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let owner_str = owner_id.to_string();

        // One transaction covers both INSERTs: either the caller gets a
        // session row with its owner participant, or neither exists.
        // Previously these were two separate calls — a failure between
        // them (DB disconnect, foreign-key violation, crash) left an
        // owner-less session that the owner couldn't rejoin because
        // ws.rs's NOT_PARTICIPANT check walks the participants table.
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO sessions (id, owner_id, target_name, input_mode, status, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&owner_str)
        .bind(target_name)
        .bind(input_mode.as_str())
        .bind(SessionStatus::Active.as_str())
        .bind(&now_str)
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
        })
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        row.map(|r| row_to_session(&r)).transpose()
    }

    async fn close_session(&self, id: &str) -> Result<()> {
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
            "UPDATE sessions SET status = ?, closed_at = ? \
             WHERE id = ? AND status = ?",
        )
        .bind(SessionStatus::Closed.as_str())
        .bind(&now_str)
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

    async fn list_active_sessions(&self) -> Result<Vec<Session>> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE status = ?")
            .bind(SessionStatus::Active.as_str())
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|r| row_to_session(&r))
            .collect::<Result<Vec<_>>>()
    }

    async fn list_sessions_for_user(&self, user_id: Uuid) -> Result<Vec<Session>> {
        let uid = user_id.to_string();
        let rows = sqlx::query(
            "SELECT DISTINCT s.* FROM sessions s \
             LEFT JOIN participants p ON p.session_id = s.id AND p.user_id = ? AND p.left_at IS NULL \
             WHERE s.status = ? AND (s.owner_id = ? OR p.user_id IS NOT NULL)",
        )
        .bind(&uid)
        .bind(SessionStatus::Active.as_str())
        .bind(&uid)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| row_to_session(&r))
            .collect::<Result<Vec<_>>>()
    }

    async fn close_stale_sessions(&self) -> Result<u64> {
        // Boot-time recovery: an unclean shutdown can leave "active"
        // sessions in the DB that no longer map to any running PTY.
        // Close them AND settle their participants in the same tx
        // so `left_at` stays consistent with the sessions row.
        let now_str = now_rfc3339();
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            "UPDATE sessions SET status = ?, closed_at = ? WHERE status = ?",
        )
        .bind(SessionStatus::Closed.as_str())
        .bind(&now_str)
        .bind(SessionStatus::Active.as_str())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE participants SET left_at = ? \
             WHERE left_at IS NULL AND session_id IN \
               (SELECT id FROM sessions WHERE status = ? AND closed_at = ?)",
        )
        .bind(&now_str)
        .bind(SessionStatus::Closed.as_str())
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(result.rows_affected())
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
        .bind(now.to_rfc3339())
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

        sqlx::query(
            "INSERT INTO invite_tokens (token_sha256, session_id, role, max_uses, used_count, expires_at) \
             VALUES (?, ?, ?, ?, 0, ?)",
        )
        .bind(&sha256_hex)
        .bind(session_id)
        .bind(role.as_str())
        .bind(max_uses)
        .bind(expires_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await?;

        let invite = InviteToken {
            token_sha256: sha256_hex,
            session_id: session_id.into(),
            role,
            max_uses,
            used_count: 0,
            expires_at,
        };
        Ok((invite, raw_token))
    }

    async fn find_invite(&self, token: &str) -> Result<InviteToken> {
        self.lookup_by_token("invite_tokens", token, row_to_invite)
            .await?
            .ok_or_else(|| Error::InvalidInput("invalid invite token".into()))
    }

    async fn consume_invite(&self, token: &str) -> Result<InviteToken> {
        let invite = self.find_invite(token).await?;

        // Atomic increment with WHERE guard — no separate validity pre-check needed
        let result = sqlx::query(
            "UPDATE invite_tokens SET used_count = used_count + 1 \
             WHERE token_sha256 = ? AND used_count < max_uses \
             AND (expires_at IS NULL OR expires_at > ?)",
        )
        .bind(&invite.token_sha256)
        .bind(now_rfc3339())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            // Determine why: expired or fully used
            check_invite_validity(&invite)?;
            // Race condition: another request consumed the last use between our lookup and UPDATE
            return Err(Error::InvalidInput(
                "invite token has been fully used".into(),
            ));
        }

        Ok(InviteToken {
            used_count: invite.used_count + 1,
            ..invite
        })
    }
}
