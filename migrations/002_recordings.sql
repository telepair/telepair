-- recordings: metadata for each recording
CREATE TABLE IF NOT EXISTS recordings (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'recording',
    file_path TEXT,
    file_size INTEGER DEFAULT 0,
    duration_ms INTEGER,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    event_count INTEGER DEFAULT 0,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    expires_at TEXT,
    created_by TEXT NOT NULL REFERENCES users(id)
);

CREATE INDEX IF NOT EXISTS idx_recordings_session_id ON recordings(session_id);
CREATE INDEX IF NOT EXISTS idx_recordings_created_by ON recordings(created_by);
CREATE INDEX IF NOT EXISTS idx_recordings_expires_at ON recordings(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status);

-- recording_shares: share tokens for recording access
CREATE TABLE IF NOT EXISTS recording_shares (
    token_sha256 TEXT PRIMARY KEY,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    max_uses INTEGER DEFAULT 0,
    used_count INTEGER DEFAULT 0,
    expires_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_recording_shares_recording_id ON recording_shares(recording_id);
