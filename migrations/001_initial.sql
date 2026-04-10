-- Telepair schema. This file is the single source of truth and is
-- applied idempotently on every boot via `sqlx::raw_sql` — every
-- statement must be safe to re-run against an already-populated DB
-- (`CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, etc.).
--
-- **In-place upgrade is supported inside the v0.1.x line.** New
-- columns added to existing tables are handled by guarded ALTER
-- statements in `SqliteStorage::run_migrations` — see that function
-- for the upgrade recipe. Pre-1.0 the wire format may still break
-- hard between minor versions, but patch releases must not require
-- "delete your DB".

CREATE TABLE IF NOT EXISTS users (
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    token_sha256       TEXT NOT NULL UNIQUE,
    is_admin           BOOLEAN NOT NULL DEFAULT FALSE,
    -- Non-null for invite-minted guests: their bearer token is valid
    -- ONLY for this one session. Every account-level route
    -- (list targets, create session, redeem a *different* invite)
    -- rejects scoped users; WS connections must target this exact
    -- session id. Null for real accounts (admins, CLI-minted users).
    -- Cannot be a real FK with ON DELETE CASCADE because sessions are
    -- soft-closed (status='closed'), never deleted — the string here
    -- is compared by equality at request time.
    scoped_session_id  TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id            TEXT PRIMARY KEY,
    owner_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_name   TEXT NOT NULL,
    input_mode    TEXT NOT NULL DEFAULT 'serialized',
    status        TEXT NOT NULL DEFAULT 'active',
    created_at    TEXT NOT NULL,
    closed_at     TEXT,
    -- Why the session was closed. Populated by the close path that
    -- stamps `closed_at`: `owner` (explicit DELETE /api/sessions/:id),
    -- `reaper` (idle-session reaper fired), `startup` (orphaned row
    -- swept on boot), or `api_error` (the close handler ran as part
    -- of error recovery). NULL on rows created before this column
    -- existed and on active sessions. See `CloseReason` in Rust.
    closed_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_owner_id ON sessions(owner_id);

CREATE TABLE IF NOT EXISTS participants (
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    joined_at   TEXT NOT NULL,
    left_at     TEXT,
    PRIMARY KEY (session_id, user_id)
);

-- list_sessions_for_user joins participants on user_id; without this
-- index it's a full table scan per call. Composite PK starts on
-- session_id so it can't serve user_id lookups.
CREATE INDEX IF NOT EXISTS idx_participants_user_id ON participants(user_id);

CREATE TABLE IF NOT EXISTS invite_tokens (
    token_sha256 TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role         TEXT NOT NULL,
    max_uses     INTEGER NOT NULL DEFAULT 1,
    used_count   INTEGER NOT NULL DEFAULT 0,
    expires_at   TEXT,
    -- RFC3339 timestamp of when the invite was minted. Nullable for
    -- rows written by v0.1.0 (there was no column then); new rows
    -- always populate it. The UI reads this for "Created N minutes
    -- ago" labels in the invite management dialog.
    created_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_invite_tokens_session_id ON invite_tokens(session_id);

-- Append-only audit log. Every row is a historical fact — rows are
-- never updated in place, only inserted. Retention is the operator's
-- problem (external `DELETE FROM audit_events WHERE ts < ...` cron);
-- Telepair itself does not prune.
--
-- `actor_id` is nullable because some events (e.g. `auth.login_failed`)
-- happen before we resolve a user identity. `actor_name` is a
-- **denormalized snapshot** captured at insertion time so the audit
-- trail survives username rewrites; joining back to the `users` table
-- is fine for "who is this now" but not for "who was this then".
-- `detail` holds an optional JSON blob with event-specific fields
-- (e.g. invite role, closed_reason, target_name) so the schema does
-- not need to grow a column for every new event type.
CREATE TABLE IF NOT EXISTS audit_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL,
    actor_id    TEXT,
    actor_name  TEXT,
    event_type  TEXT NOT NULL,
    session_id  TEXT,
    detail      TEXT
);

-- Four indexes: the audit-query CLI and the session-detail view both
-- hit this table with time-windowed filters on one of four axes.
-- Without these, every `telepair admin audit --last 24h` scans the
-- entire table. Each index is DESC on `ts` so "recent first" results
-- land in index order and skip the sort step.
CREATE INDEX IF NOT EXISTS idx_audit_ts      ON audit_events(ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_session ON audit_events(session_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_actor   ON audit_events(actor_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_type    ON audit_events(event_type, ts DESC);
