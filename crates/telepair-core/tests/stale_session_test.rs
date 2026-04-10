use telepair_core::session::{CloseReason, InputMode, SessionStatus};
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> SqliteStorage {
    SqliteStorage::new_memory().await.unwrap()
}

#[tokio::test]
async fn close_stale_sessions_returns_count() {
    let store = setup().await;
    let (user, _) = store.create_user("alice", false).await.unwrap();

    store
        .create_session_with_owner(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();
    store
        .create_session_with_owner(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    let count = store
        .close_stale_sessions(CloseReason::Startup)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn close_stale_sessions_zero_when_none_active() {
    let store = setup().await;
    let count = store
        .close_stale_sessions(CloseReason::Startup)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn close_stale_sessions_marks_as_closed() {
    let store = setup().await;
    let (user, _) = store.create_user("bob", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    store
        .close_stale_sessions(CloseReason::Startup)
        .await
        .unwrap();

    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Closed);
    assert!(fetched.closed_at.is_some());
    assert_eq!(fetched.closed_reason, Some(CloseReason::Startup));
}

#[tokio::test]
async fn close_stale_sessions_skips_already_closed() {
    let store = setup().await;
    let (user, _) = store.create_user("carol", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    // Close manually first
    store
        .close_session(&session.id, CloseReason::Owner)
        .await
        .unwrap();

    // Stale cleanup should find 0
    let count = store
        .close_stale_sessions(CloseReason::Startup)
        .await
        .unwrap();
    assert_eq!(count, 0);

    // Owner reason sticks — the no-op stale pass doesn't overwrite it.
    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.closed_reason, Some(CloseReason::Owner));
}
