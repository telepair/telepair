use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> SqliteStorage {
    SqliteStorage::new_memory().await.unwrap()
}

#[tokio::test]
async fn create_and_get_user() {
    let store = setup().await;
    let (user, token) = store.create_user("alice", true).await.unwrap();
    assert_eq!(user.name, "alice");
    assert!(user.is_admin);
    assert!(!token.is_empty());

    let fetched = store.get_user(user.id).await.unwrap().unwrap();
    assert_eq!(fetched.name, "alice");
}

#[tokio::test]
async fn validate_token() {
    let store = setup().await;
    let (user, token) = store.create_user("bob", false).await.unwrap();
    let validated = store.validate_token(&token).await.unwrap();
    assert_eq!(validated.id, user.id);
}

#[tokio::test]
async fn invalid_token_fails() {
    let store = setup().await;
    store.create_user("carol", false).await.unwrap();
    let result = store.validate_token("wrong-token").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn create_and_get_session() {
    let store = setup().await;
    let (user, _) = store.create_user("dave", false).await.unwrap();
    let session = store
        .create_session(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    assert_eq!(session.target_name, "local-shell");

    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, session.id);
}

#[tokio::test]
async fn add_and_list_participants() {
    let store = setup().await;
    let (owner, _) = store.create_user("eve", false).await.unwrap();
    let (viewer, _) = store.create_user("frank", false).await.unwrap();
    let session = store
        .create_session(owner.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    store
        .upsert_participant(&session.id, owner.id, Role::Owner)
        .await
        .unwrap();
    store
        .upsert_participant(&session.id, viewer.id, Role::Viewer)
        .await
        .unwrap();

    let participants = store.list_participants(&session.id).await.unwrap();
    assert_eq!(participants.len(), 2);
}

#[tokio::test]
async fn close_session() {
    let store = setup().await;
    let (user, _) = store.create_user("grace", false).await.unwrap();
    let session = store
        .create_session(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    store.close_session(&session.id).await.unwrap();
    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(
        fetched.status,
        telepair_core::session::SessionStatus::Closed
    );
    assert!(fetched.closed_at.is_some());
}

/// Simulate an older release's schema (pre-`token_sha256`) and confirm that
/// `SqliteStorage::new` refuses to boot instead of silently serving broken
/// auth lookups against the stale table.
#[tokio::test]
async fn rejects_outdated_schema_missing_token_sha256_column() {
    // Seed a fresh in-memory DB with the old (pre-sha256) users table,
    // then point SqliteStorage at the same pool via a shared cache URI.
    let uri = "file:outdated_schema_test?mode=memory&cache=shared".to_string();
    let options: SqliteConnectOptions = uri.parse().unwrap();
    let seed_pool = SqlitePool::connect_with(options).await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            token_hash TEXT NOT NULL,
            is_admin BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )
    .execute(&seed_pool)
    .await
    .unwrap();

    let result = SqliteStorage::new(&uri).await;
    match result {
        Ok(_) => panic!("stale schema should have been rejected"),
        Err(Error::SchemaOutdated(msg)) => {
            assert!(
                msg.contains("users"),
                "error should name the affected table: {msg}"
            );
            assert!(
                msg.contains("token_sha256"),
                "error should name the missing column: {msg}"
            );
        }
        Err(other) => panic!("expected SchemaOutdated, got: {other:?}"),
    }

    drop(seed_pool);
}

/// Simulate the shape shipped by the *previous* release (column present but
/// nullable) with all rows already populated. This must boot cleanly —
/// rejecting it would force operators to wipe their DB during an in-place
/// upgrade.
#[tokio::test]
async fn accepts_previous_release_nullable_schema_with_populated_data() {
    let uri = "file:prev_release_schema_test?mode=memory&cache=shared".to_string();
    let options: SqliteConnectOptions = uri.parse().unwrap();
    let seed_pool = SqlitePool::connect_with(options).await.unwrap();

    // Previous release DDL: token_sha256 nullable + partial unique index.
    sqlx::raw_sql(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            token_hash TEXT NOT NULL,
            token_sha256 TEXT,
            is_admin BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX idx_users_token_sha256
            ON users(token_sha256) WHERE token_sha256 IS NOT NULL;
        INSERT INTO users (id, name, token_hash, token_sha256, is_admin, created_at, updated_at)
        VALUES ('u1', 'legacy', 'bcrypt$dummy', 'abc123', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            owner_id TEXT NOT NULL REFERENCES users(id),
            target_name TEXT NOT NULL,
            input_mode TEXT NOT NULL DEFAULT 'serialized',
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            closed_at TEXT
        );
        CREATE TABLE invite_tokens (
            token_hash TEXT PRIMARY KEY,
            token_sha256 TEXT,
            session_id TEXT NOT NULL REFERENCES sessions(id),
            role TEXT NOT NULL,
            max_uses INTEGER NOT NULL DEFAULT 1,
            used_count INTEGER NOT NULL DEFAULT 0,
            expires_at TEXT
        );",
    )
    .execute(&seed_pool)
    .await
    .unwrap();

    let result = SqliteStorage::new(&uri).await;
    assert!(
        result.is_ok(),
        "previous release shape with populated data should boot cleanly, got {:?}",
        result.err()
    );

    drop(seed_pool);
}

/// A database left over from a release where `token_sha256` existed but was
/// never populated for every row must still be rejected — otherwise those
/// NULL rows would silently fail every SHA-256 lookup.
#[tokio::test]
async fn rejects_previous_release_schema_with_null_rows() {
    let uri = "file:prev_release_null_rows_test?mode=memory&cache=shared".to_string();
    let options: SqliteConnectOptions = uri.parse().unwrap();
    let seed_pool = SqlitePool::connect_with(options).await.unwrap();

    sqlx::raw_sql(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            token_hash TEXT NOT NULL,
            token_sha256 TEXT,
            is_admin BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        INSERT INTO users (id, name, token_hash, token_sha256, is_admin, created_at, updated_at)
        VALUES ('u1', 'orphan', 'bcrypt$dummy', NULL, 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
    )
    .execute(&seed_pool)
    .await
    .unwrap();

    let result = SqliteStorage::new(&uri).await;
    match result {
        Ok(_) => panic!("NULL token_sha256 rows should have been rejected"),
        Err(Error::SchemaOutdated(msg)) => {
            assert!(msg.contains("NULL"), "error should mention NULL: {msg}");
            assert!(
                msg.contains("token_sha256"),
                "error should name the column: {msg}"
            );
        }
        Err(other) => panic!("expected SchemaOutdated, got: {other:?}"),
    }

    drop(seed_pool);
}

#[tokio::test]
async fn create_session_with_owner_is_atomic() {
    // Happy path: the session row and the owner participant row must
    // land together. Previously these were two separate non-txn writes
    // from SessionService, so a failure between them would leave an
    // owner-less session.
    let store = setup().await;
    let (user, _) = store.create_user("atomic", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();

    // Both rows must be present in the same "moment".
    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.owner_id, user.id);

    let participants = store.list_participants(&session.id).await.unwrap();
    assert_eq!(
        participants.len(),
        1,
        "owner participant should be inserted in the same transaction"
    );
    assert_eq!(participants[0].user_id, user.id);
    assert_eq!(participants[0].role, Role::Owner);
}

#[tokio::test]
async fn create_session_with_owner_rolls_back_on_fk_violation() {
    // Force the second INSERT to fail by passing an owner_id that
    // doesn't exist in users. The sessions table declares
    // `owner_id REFERENCES users(id)`, so the first INSERT fails FK
    // and the whole transaction rolls back — crucially, NO session
    // row should be visible afterwards.
    let store = setup().await;
    let ghost_owner = uuid::Uuid::new_v4();
    let result = store
        .create_session_with_owner(ghost_owner, "local-shell", InputMode::Serialized)
        .await;
    assert!(
        result.is_err(),
        "creating a session for a nonexistent owner must fail"
    );

    // Nothing should be hanging around from the rolled-back tx.
    let active = store.list_active_sessions().await.unwrap();
    assert!(
        active.is_empty(),
        "rolled-back transaction must not leave session rows: {active:?}"
    );
}

#[tokio::test]
async fn corrupt_closed_at_is_reported_not_silently_dropped() {
    // The old row_to_session used `and_then(|s| s.parse().ok())` which
    // quietly turned an unparseable closed_at into None — making a
    // corrupt row look "still open" to every caller. The fix should
    // surface an error instead.
    let uri = "file:corrupt_closed_at_test?mode=memory&cache=shared".to_string();
    let options: SqliteConnectOptions = uri.parse().unwrap();
    let seed_pool = SqlitePool::connect_with(options).await.unwrap();

    // Let SqliteStorage run its migrations on the shared DB first, so
    // we have a well-formed schema to poison after the fact.
    let store = SqliteStorage::new(&uri).await.unwrap();
    let (user, _) = store.create_user("victim", false).await.unwrap();
    let session = store
        .create_session(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();

    // Poke garbage into closed_at. The column is TEXT so SQLite will
    // happily store this; only our row parser should complain.
    sqlx::query("UPDATE sessions SET closed_at = ? WHERE id = ?")
        .bind("not-a-timestamp")
        .bind(&session.id)
        .execute(&seed_pool)
        .await
        .unwrap();

    let result = store.get_session(&session.id).await;
    match result {
        Ok(_) => panic!("corrupt closed_at must surface an error, not be silently dropped"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("closed_at"),
                "error should mention the offending column, got: {msg}"
            );
        }
    }

    drop(seed_pool);
}
