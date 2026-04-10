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
    CloseReason, InputMode, InviteToken, Participant, Session, SessionListFilter, SessionStatus,
    User,
};
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

        Ok(())
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

fn row_to_user(r: &SqliteRow) -> Result<User> {
    Ok(User {
        id: parse_uuid(r.get("id"))?,
        name: r.get("name"),
        is_admin: r.get("is_admin"),
        scoped_session_id: r.get("scoped_session_id"),
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
        let row = sqlx::query(
            "SELECT id, name, is_admin, scoped_session_id, created_at, updated_at \
             FROM users WHERE id = ?",
        )
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
               (id, name, token_sha256, is_admin, scoped_session_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        let row = sqlx::query(
            "SELECT id, name, is_admin, scoped_session_id, created_at, updated_at \
             FROM users WHERE name = ?",
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
            closed_reason: None,
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

    async fn close_stale_sessions(&self, reason: CloseReason) -> Result<u64> {
        // Boot-time recovery: an unclean shutdown can leave "active"
        // sessions in the DB that no longer map to any running PTY.
        // Close them AND settle their participants in the same tx
        // so `left_at` stays consistent with the sessions row.
        let now_str = now_rfc3339();
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            "UPDATE sessions SET status = ?, closed_at = ?, closed_reason = ? WHERE status = ?",
        )
        .bind(SessionStatus::Closed.as_str())
        .bind(&now_str)
        .bind(reason.as_str())
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
}
