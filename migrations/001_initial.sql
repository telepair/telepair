-- Initial schema. Pre-1.0: no migration compatibility shims. If a
-- dev DB exists from a previous release, delete ~/.telepair/telepair.db
-- and let the server recreate it on startup.

CREATE TABLE IF NOT EXISTS users (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    token_sha256 TEXT NOT NULL UNIQUE,
    is_admin     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_name TEXT NOT NULL,
    input_mode  TEXT NOT NULL DEFAULT 'serialized',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL,
    closed_at   TEXT
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
    expires_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_invite_tokens_session_id ON invite_tokens(session_id);
