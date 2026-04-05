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
        // Enable WAL mode for better concurrent read performance
        sqlx::raw_sql("PRAGMA journal_mode=WAL;")
            .execute(&self.pool)
            .await?;

        // Run base schema (CREATE TABLE IF NOT EXISTS)
        sqlx::raw_sql(include_str!("../../../../migrations/001_initial.sql"))
            .execute(&self.pool)
            .await?;

        // Migration: add token_sha256 column to users (idempotent)
        let has_users_col: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('users') WHERE name = 'token_sha256'",
        )
        .fetch_one(&self.pool)
        .await?;

        if !has_users_col {
            sqlx::raw_sql(
                "ALTER TABLE users ADD COLUMN token_sha256 TEXT;\
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_users_token_sha256 \
                     ON users(token_sha256) WHERE token_sha256 IS NOT NULL;",
            )
            .execute(&self.pool)
            .await?;
        }

        // Migration: add token_sha256 column to invite_tokens (idempotent)
        let has_invite_col: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('invite_tokens') WHERE name = 'token_sha256'",
        )
        .fetch_one(&self.pool)
        .await?;

        if !has_invite_col {
            sqlx::raw_sql(
                "ALTER TABLE invite_tokens ADD COLUMN token_sha256 TEXT;\
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_invite_tokens_token_sha256 \
                     ON invite_tokens(token_sha256) WHERE token_sha256 IS NOT NULL;",
            )
            .execute(&self.pool)
            .await?;
        }

        // Index on sessions.status for faster list_active_sessions
        sqlx::raw_sql(
            "CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);",
        )
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

fn row_to_user(r: &SqliteRow) -> Result<User> {
    Ok(User {
        id: parse_uuid(r.get("id"))?,
        name: r.get("name"),
        is_admin: r.get("is_admin"),
        created_at: parse_datetime(r.get("created_at"))?,
        updated_at: parse_datetime(r.get("updated_at"))?,
    })
}

fn row_to_session(r: &SqliteRow) -> Result<Session> {
    Ok(Session {
        id: r.get("id"),
        owner_id: parse_uuid(r.get("owner_id"))?,
        target_name: r.get("target_name"),
        input_mode: r
            .get::<String, _>("input_mode")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        status: r
            .get::<String, _>("status")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        created_at: parse_datetime(r.get("created_at"))?,
        closed_at: r
            .get::<Option<String>, _>("closed_at")
            .and_then(|s| s.parse().ok()),
    })
}

fn row_to_participant(r: &SqliteRow) -> Result<Participant> {
    Ok(Participant {
        session_id: r.get("session_id"),
        user_id: parse_uuid(r.get("user_id"))?,
        role: r
            .get::<String, _>("role")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        joined_at: parse_datetime(r.get("joined_at"))?,
        left_at: r
            .get::<Option<String>, _>("left_at")
            .and_then(|s| s.parse().ok()),
    })
}

fn row_to_invite(r: &SqliteRow) -> Result<InviteToken> {
    Ok(InviteToken {
        token_hash: r.get("token_hash"),
        session_id: r.get("session_id"),
        role: r
            .get::<String, _>("role")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        max_uses: r.get("max_uses"),
        used_count: r.get("used_count"),
        expires_at: r
            .get::<Option<String>, _>("expires_at")
            .and_then(|s| s.parse().ok()),
    })
}

const BCRYPT_COST: u32 = 10;

/// Compute SHA-256 hex digest of a raw token for O(1) indexed lookup.
fn token_sha256(raw: &str) -> String {
    let hash = Sha256::digest(raw.as_bytes());
    hex::encode(hash)
}

/// Generate a new token, returning (raw, bcrypt_hash, sha256_hex).
fn generate_token() -> Result<(String, String, String)> {
    let raw = nanoid::nanoid!(32);
    let bcrypt_hash = bcrypt::hash(&raw, BCRYPT_COST).map_err(|e| Error::Auth(e.to_string()))?;
    let sha256_hex = token_sha256(&raw);
    Ok((raw, bcrypt_hash, sha256_hex))
}

fn check_invite_validity(invite: &InviteToken) -> Result<()> {
    if let Some(expires_at) = invite.expires_at {
        if expires_at < Utc::now() {
            return Err(Error::Auth("invite token has expired".into()));
        }
    }
    if invite.used_count >= invite.max_uses {
        return Err(Error::Auth("invite token has been fully used".into()));
    }
    Ok(())
}

impl SqliteStorage {
    /// Look up an invite by raw token (SHA-256 fast path + bcrypt legacy fallback).
    /// Does NOT check expiry or usage limits.
    async fn find_invite_by_token(&self, token: &str) -> Result<InviteToken> {
        let sha256_hex = token_sha256(token);

        // Fast path: O(1) indexed lookup by SHA-256
        if let Some(row) = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 = ?")
            .bind(&sha256_hex)
            .fetch_optional(&self.pool)
            .await?
        {
            return row_to_invite(&row);
        }

        // Slow path: legacy invite tokens without token_sha256 — bcrypt scan
        let rows = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 IS NULL")
            .fetch_all(&self.pool)
            .await?;

        for row in rows {
            let hash: String = row.get("token_hash");
            if bcrypt::verify(token, &hash).unwrap_or(false) {
                // Backfill token_sha256 for future O(1) lookups
                let token_hash_val: String = row.get("token_hash");
                let _ = sqlx::query(
                    "UPDATE invite_tokens SET token_sha256 = ? WHERE token_hash = ?",
                )
                .bind(&sha256_hex)
                .bind(&token_hash_val)
                .execute(&self.pool)
                .await;
                return row_to_invite(&row);
            }
        }

        Err(Error::Auth("invalid invite token".into()))
    }
}

impl Storage for SqliteStorage {
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)> {
        let id = Uuid::new_v4();
        let (token, token_hash, sha256_hex) = generate_token()?;
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO users (id, name, token_hash, token_sha256, is_admin, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(&token_hash)
        .bind(&sha256_hex)
        .bind(is_admin)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
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

    async fn get_user(&self, id: Uuid) -> Result<Option<User>> {
        let row = sqlx::query(
            "SELECT id, name, is_admin, created_at, updated_at FROM users WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| row_to_user(&r)).transpose()
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
        let sha256_hex = token_sha256(token);

        // Fast path: O(1) indexed lookup by SHA-256
        if let Some(row) = sqlx::query(
            "SELECT id, name, is_admin, created_at, updated_at FROM users WHERE token_sha256 = ?",
        )
        .bind(&sha256_hex)
        .fetch_optional(&self.pool)
        .await?
        {
            return row_to_user(&row);
        }

        // Slow path: legacy tokens without token_sha256 — bcrypt scan only those rows
        let rows = sqlx::query(
            "SELECT id, name, token_hash, is_admin, created_at, updated_at FROM users WHERE token_sha256 IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        for row in rows {
            let hash: String = row.get("token_hash");
            if bcrypt::verify(token, &hash).unwrap_or(false) {
                // Backfill token_sha256 for future O(1) lookups
                let user_id: String = row.get("id");
                let _ = sqlx::query("UPDATE users SET token_sha256 = ? WHERE id = ?")
                    .bind(&sha256_hex)
                    .bind(&user_id)
                    .execute(&self.pool)
                    .await;
                return row_to_user(&row);
            }
        }

        Err(Error::Auth("invalid token".into()))
    }

    async fn create_session(
        &self,
        owner_id: Uuid,
        target_name: &str,
        input_mode: InputMode,
    ) -> Result<Session> {
        let id = nanoid::nanoid!(10);
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO sessions (id, owner_id, target_name, input_mode, status, created_at) VALUES (?, ?, ?, ?, 'active', ?)",
        )
        .bind(&id)
        .bind(owner_id.to_string())
        .bind(target_name)
        .bind(input_mode.as_str())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

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
        let now = Utc::now();
        let result =
            sqlx::query("UPDATE sessions SET status = 'closed', closed_at = ? WHERE id = ? AND status = 'active'")
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(Error::SessionNotFound(id.to_string()));
        }
        Ok(())
    }

    async fn list_active_sessions(&self) -> Result<Vec<Session>> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE status = 'active'")
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
             WHERE s.status = 'active' AND (s.owner_id = ? OR p.user_id IS NOT NULL)",
        )
        .bind(&uid)
        .bind(&uid)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| row_to_session(&r))
            .collect::<Result<Vec<_>>>()
    }

    async fn close_stale_sessions(&self) -> Result<u64> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE sessions SET status = 'closed', closed_at = ? WHERE status = 'active'",
        )
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn add_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        role: Role,
    ) -> Result<Participant> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO participants (session_id, user_id, role, joined_at) VALUES (?, ?, ?, ?)",
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

    async fn remove_participant(&self, session_id: &str, user_id: Uuid) -> Result<()> {
        let now = Utc::now();
        sqlx::query(
            "UPDATE participants SET left_at = ? WHERE session_id = ? AND user_id = ?",
        )
        .bind(now.to_rfc3339())
        .bind(session_id)
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>> {
        let rows = sqlx::query(
            "SELECT * FROM participants WHERE session_id = ? AND left_at IS NULL",
        )
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
            return Err(Error::InvalidInput(
                "max_uses must be at least 1".into(),
            ));
        }
        let (raw_token, token_hash, sha256_hex) = generate_token()?;

        sqlx::query(
            "INSERT INTO invite_tokens (token_hash, token_sha256, session_id, role, max_uses, used_count, expires_at) VALUES (?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&token_hash)
        .bind(&sha256_hex)
        .bind(session_id)
        .bind(role.as_str())
        .bind(max_uses)
        .bind(expires_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await?;

        let invite = InviteToken {
            token_hash,
            session_id: session_id.into(),
            role,
            max_uses,
            used_count: 0,
            expires_at,
        };
        Ok((invite, raw_token))
    }

    async fn validate_invite(&self, token: &str) -> Result<InviteToken> {
        let invite = self.find_invite_by_token(token).await?;
        check_invite_validity(&invite)?;
        Ok(invite)
    }

    async fn consume_invite(&self, token: &str) -> Result<InviteToken> {
        let invite = self.find_invite_by_token(token).await?;

        // Atomic increment with WHERE guard — no separate validity pre-check needed
        let result = sqlx::query(
            "UPDATE invite_tokens SET used_count = used_count + 1 \
             WHERE token_hash = ? AND used_count < max_uses \
             AND (expires_at IS NULL OR expires_at > ?)",
        )
        .bind(&invite.token_hash)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            // Determine why: expired or fully used
            check_invite_validity(&invite)?;
            // Race condition: another request consumed the last use between our lookup and UPDATE
            return Err(Error::Auth("invite token has been fully used".into()));
        }

        Ok(InviteToken {
            used_count: invite.used_count + 1,
            ..invite
        })
    }
}
