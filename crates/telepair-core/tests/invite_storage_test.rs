use chrono::{Duration, Utc};
use telepair_core::permission::Role;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> (SqliteStorage, String) {
    let store = SqliteStorage::new_memory().await.unwrap();
    let (user, _) = store.create_user("host", false).await.unwrap();
    let session = store
        .create_session(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();
    (store, session.id)
}

#[tokio::test]
async fn create_and_validate_invite_token() {
    let (store, session_id) = setup().await;

    let (invite, raw_token) = store
        .create_invite(&session_id, Role::Operator, 5, None)
        .await
        .unwrap();

    assert_eq!(invite.session_id, session_id);
    assert_eq!(invite.role, Role::Operator);
    assert_eq!(invite.max_uses, 5);
    assert_eq!(invite.used_count, 0);
    assert!(invite.expires_at.is_none());
    assert!(!raw_token.is_empty());

    let validated = store.validate_invite(&raw_token).await.unwrap();
    assert_eq!(validated.session_id, session_id);
    assert_eq!(validated.role, Role::Operator);
    assert_eq!(validated.max_uses, 5);
}

#[tokio::test]
async fn consume_invite_token() {
    let (store, session_id) = setup().await;

    let (_, raw_token) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    let consumed = store.consume_invite(&raw_token).await.unwrap();
    assert_eq!(consumed.used_count, 1);

    let result = store.consume_invite(&raw_token).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn expired_invite_token_rejected() {
    let (store, session_id) = setup().await;

    let past = Utc::now() - Duration::hours(1);
    let (_, raw_token) = store
        .create_invite(&session_id, Role::Viewer, 10, Some(past))
        .await
        .unwrap();

    let result = store.consume_invite(&raw_token).await;
    assert!(result.is_err());
}
