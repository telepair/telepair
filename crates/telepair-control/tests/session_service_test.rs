use std::sync::Arc;
use telepair_control::session_service::SessionService;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> (SessionService, String) {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, token) = store.create_user("owner", false).await.unwrap();
    let svc = SessionService::new(store);
    (svc, token)
}

#[tokio::test]
async fn create_session_adds_owner_as_participant() {
    let (svc, token) = setup().await;
    let user = svc.storage().validate_token(&token).await.unwrap();
    let session = svc
        .create_session(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();

    let participants = svc.storage().list_participants(&session.id).await.unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].role, telepair_core::permission::Role::Owner);
}

#[tokio::test]
async fn close_session_updates_status() {
    let (svc, token) = setup().await;
    let user = svc.storage().validate_token(&token).await.unwrap();
    let session = svc
        .create_session(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    svc.close_session(&session.id).await.unwrap();
    let fetched = svc
        .storage()
        .get_session(&session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fetched.status,
        telepair_core::session::SessionStatus::Closed
    );
}

#[tokio::test]
async fn list_active_sessions() {
    let (svc, token) = setup().await;
    let user = svc.storage().validate_token(&token).await.unwrap();
    svc.create_session(user.id, "s1", InputMode::Serialized)
        .await
        .unwrap();
    svc.create_session(user.id, "s2", InputMode::Multiplexed)
        .await
        .unwrap();

    let sessions = svc.list_active_sessions().await.unwrap();
    assert_eq!(sessions.len(), 2);
}
