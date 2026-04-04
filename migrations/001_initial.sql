CREATE TABLE IF NOT EXISTS users (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    token_hash  TEXT NOT NULL,
    token_sha256 TEXT,
    is_admin    BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_token_sha256
    ON users(token_sha256) WHERE token_sha256 IS NOT NULL;

CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id),
    target_name TEXT NOT NULL,
    input_mode  TEXT NOT NULL DEFAULT 'serialized',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL,
    closed_at   TEXT
);

CREATE TABLE IF NOT EXISTS participants (
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    user_id     TEXT NOT NULL REFERENCES users(id),
    role        TEXT NOT NULL,
    joined_at   TEXT NOT NULL,
    left_at     TEXT,
    PRIMARY KEY (session_id, user_id)
);

CREATE TABLE IF NOT EXISTS invite_tokens (
    token_hash  TEXT PRIMARY KEY,
    token_sha256 TEXT,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    role        TEXT NOT NULL,
    max_uses    INTEGER NOT NULL DEFAULT 1,
    used_count  INTEGER NOT NULL DEFAULT 0,
    expires_at  TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_invite_tokens_token_sha256
    ON invite_tokens(token_sha256) WHERE token_sha256 IS NOT NULL;
