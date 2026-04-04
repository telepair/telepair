use std::str::FromStr;

use chrono::Utc;
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
        sqlx::raw_sql(include_str!("../../../../migrations/001_initial.sql"))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_user(r: &SqliteRow) -> Result<User> {
    Ok(User {
        id: r
            .get::<String, _>("id")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid user id: {e}")))?,
        name: r.get("name"),
        is_admin: r.get("is_admin"),
        created_at: r
            .get::<String, _>("created_at")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid timestamp: {e}")))?,
        updated_at: r
            .get::<String, _>("updated_at")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid timestamp: {e}")))?,
    })
}

fn row_to_session(r: &SqliteRow) -> Result<Session> {
    Ok(Session {
        id: r.get("id"),
        owner_id: r
            .get::<String, _>("owner_id")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid user id: {e}")))?,
        target_name: r.get("target_name"),
        input_mode: r
            .get::<String, _>("input_mode")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        status: r
            .get::<String, _>("status")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        created_at: r
            .get::<String, _>("created_at")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid timestamp: {e}")))?,
        closed_at: r
            .get::<Option<String>, _>("closed_at")
            .and_then(|s| s.parse().ok()),
    })
}

fn row_to_participant(r: &SqliteRow) -> Result<Participant> {
    Ok(Participant {
        session_id: r.get("session_id"),
        user_id: r
            .get::<String, _>("user_id")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid user id: {e}")))?,
        role: r
            .get::<String, _>("role")
            .parse()
            .map_err(|e: String| Error::InvalidInput(e))?,
        joined_at: r
            .get::<String, _>("joined_at")
            .parse()
            .map_err(|e| Error::InvalidInput(format!("invalid timestamp: {e}")))?,
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

fn generate_token() -> Result<(String, String)> {
    let raw = nanoid::nanoid!(32);
    let hash = bcrypt::hash(&raw, BCRYPT_COST).map_err(|e| Error::Auth(e.to_string()))?;
    Ok((raw, hash))
}

impl Storage for SqliteStorage {
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)> {
        let id = Uuid::new_v4();
        let (token, token_hash) = generate_token()?;
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO users (id, name, token_hash, is_admin, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(&token_hash)
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
        let rows = sqlx::query(
            "SELECT id, name, token_hash, is_admin, created_at, updated_at FROM users",
        )
        .fetch_all(&self.pool)
        .await?;

        for row in rows {
            let hash: String = row.get("token_hash");
            if bcrypt::verify(token, &hash).unwrap_or(false) {
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
            sqlx::query("UPDATE sessions SET status = 'closed', closed_at = ? WHERE id = ?")
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
        let (raw_token, token_hash) = generate_token()?;

        sqlx::query(
            "INSERT INTO invite_tokens (token_hash, session_id, role, max_uses, used_count, expires_at) VALUES (?, ?, ?, ?, 0, ?)",
        )
        .bind(&token_hash)
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
        let rows = sqlx::query("SELECT * FROM invite_tokens")
            .fetch_all(&self.pool)
            .await?;

        for row in rows {
            let hash: String = row.get("token_hash");
            if bcrypt::verify(token, &hash).unwrap_or(false) {
                return row_to_invite(&row);
            }
        }
        Err(Error::Auth("invalid invite token".into()))
    }

    async fn consume_invite(&self, token: &str) -> Result<InviteToken> {
        let invite = self.validate_invite(token).await?;

        if let Some(expires_at) = invite.expires_at {
            if expires_at < Utc::now() {
                return Err(Error::Auth("invite token has expired".into()));
            }
        }

        // Atomic increment with WHERE guard to prevent TOCTOU race
        let result = sqlx::query(
            "UPDATE invite_tokens SET used_count = used_count + 1 WHERE token_hash = ? AND used_count < max_uses",
        )
        .bind(&invite.token_hash)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(Error::Auth("invite token has been fully used".into()));
        }

        Ok(InviteToken {
            used_count: invite.used_count + 1,
            ..invite
        })
    }
}
