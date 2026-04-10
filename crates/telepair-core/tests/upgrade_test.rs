//! Upgrade tests: simulate a v0.1.0 DB on disk, then point
//! `SqliteStorage::new` at it and verify the migration runner adds
//! the new `sessions.closed_reason` column and the `audit_events`
//! table without touching any pre-existing rows.
//!
//! These tests are the safety net that keeps the pre-1.0 "delete your
//! DB" footgun out of the v0.1.x line — patch releases must handle
//! in-place upgrades. If this test starts failing, do not loosen it:
//! fix the migration runner so the upgrade path stays clean.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use tempfile::tempdir;

use telepair_core::storage::SqliteStorage;

/// The exact v0.1.0 schema, prior to this release's additions.
/// Kept verbatim as a string so we can seed a "pretend previous
/// version" DB without standing up an old git checkout.
const V010_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    token_sha256       TEXT NOT NULL UNIQUE,
    is_admin           BOOLEAN NOT NULL DEFAULT FALSE,
    scoped_session_id  TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
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
"#;

async fn seed_v010_db(url: &str) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(url)
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(V010_SCHEMA).execute(&pool).await.unwrap();
    pool
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
    let rows = sqlx::query("SELECT name FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(pool)
        .await
        .unwrap();
    rows.iter().any(|r| r.get::<String, _>("name") == column)
}

async fn table_exists(pool: &SqlitePool, table: &str) -> bool {
    let row = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name = ?")
        .bind(table)
        .fetch_optional(pool)
        .await
        .unwrap();
    row.is_some()
}

#[tokio::test]
async fn upgrade_from_v010_adds_closed_reason_and_audit_events() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("telepair.db");
    let url = format!("sqlite://{}", db_path.display());

    // Seed a DB that looks like v0.1.0 — has users/sessions/
    // participants/invite_tokens, missing closed_reason and audit.
    let seed_pool = seed_v010_db(&url).await;
    assert!(!column_exists(&seed_pool, "sessions", "closed_reason").await);
    assert!(!table_exists(&seed_pool, "audit_events").await);
    seed_pool.close().await;

    // Now hand it to the v0.1.1 loader; run_migrations() must lift
    // it to the current shape without destroying anything.
    let storage = SqliteStorage::new(&url).await.unwrap();
    drop(storage); // we only need the side-effects of run_migrations

    // Re-open with raw sqlx to introspect.
    let options = SqliteConnectOptions::from_str(&url).unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();

    assert!(
        column_exists(&pool, "sessions", "closed_reason").await,
        "closed_reason must be added to sessions after upgrade"
    );
    assert!(
        table_exists(&pool, "audit_events").await,
        "audit_events table must be created on upgrade"
    );
}

#[tokio::test]
async fn upgrade_preserves_existing_rows() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("telepair.db");
    let url = format!("sqlite://{}", db_path.display());

    // Seed v0.1.0 schema + insert a row so we can prove the ALTER
    // did not drop or rewrite anything.
    let seed_pool = seed_v010_db(&url).await;
    sqlx::query(
        "INSERT INTO users (id, name, token_sha256, is_admin, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind("00000000-0000-0000-0000-000000000001")
    .bind("alice")
    .bind("abc123")
    .bind(true)
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(&seed_pool)
    .await
    .unwrap();
    seed_pool.close().await;

    // Run the upgrade.
    let storage = SqliteStorage::new(&url).await.unwrap();
    drop(storage);

    // The pre-existing row must still be there, unchanged.
    let options = SqliteConnectOptions::from_str(&url).unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let row = sqlx::query("SELECT name, is_admin FROM users WHERE id = ?")
        .bind("00000000-0000-0000-0000-000000000001")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("name"), "alice");
    assert!(row.get::<bool, _>("is_admin"));
}

#[tokio::test]
async fn run_migrations_is_idempotent() {
    // Booting the same DB twice must not error — a restart of the
    // telepair process is the most common upgrade trigger, and the
    // migration runner must not double-apply or blow up on the
    // "column already exists" path.
    //
    // `mode=rwc` mirrors how `telepair-cli::main` opens the real
    // on-disk DB (it creates the file on first boot). Without that
    // query param sqlx refuses to create the file.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("telepair.db");
    let url = format!("sqlite:{}?mode=rwc", db_path.display());

    let first = SqliteStorage::new(&url).await.unwrap();
    drop(first);
    let second = SqliteStorage::new(&url).await.unwrap();
    drop(second);
}
