use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
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
