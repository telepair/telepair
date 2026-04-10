use chrono::{Duration, Utc};
use telepair_core::permission::Role;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> (SqliteStorage, String) {
    let store = SqliteStorage::new_memory().await.unwrap();
    let (user, _) = store.create_user("host", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();
    (store, session.id)
}

#[tokio::test]
async fn create_and_consume_invite_token() {
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

    // Consume once — verifies lookup by raw token works and increments counter
    let consumed = store.consume_invite(&raw_token).await.unwrap();
    assert_eq!(consumed.session_id, session_id);
    assert_eq!(consumed.role, Role::Operator);
    assert_eq!(consumed.max_uses, 5);
    assert_eq!(consumed.used_count, 1);
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

#[tokio::test]
async fn create_invite_stamps_created_at() {
    let (store, session_id) = setup().await;

    let before = Utc::now();
    let (invite, _) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();
    let after = Utc::now();

    let created_at = invite.created_at.expect("created_at should be populated");
    assert!(created_at >= before - Duration::seconds(1));
    assert!(created_at <= after + Duration::seconds(1));
}

#[tokio::test]
async fn list_invites_returns_rows_in_desc_order() {
    let (store, session_id) = setup().await;

    // Three invites with distinguishable roles so we can eyeball ordering.
    let (_a, _) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();
    // Ensure non-identical created_at timestamps even on fast machines.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (_b, _) = store
        .create_invite(&session_id, Role::Operator, 3, None)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let (_c, _) = store
        .create_invite(&session_id, Role::Viewer, 2, None)
        .await
        .unwrap();

    let rows = store.list_invites_for_session(&session_id).await.unwrap();
    assert_eq!(rows.len(), 3);
    // Newest-first ordering keeps the management dialog consistent
    // regardless of how many invites were minted in a single second.
    assert!(rows[0].created_at >= rows[1].created_at);
    assert!(rows[1].created_at >= rows[2].created_at);
}

#[tokio::test]
async fn list_invites_includes_exhausted_and_expired() {
    let (store, session_id) = setup().await;

    // Fresh, unused invite.
    let (_, _) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    // Exhausted invite: create, consume, assert it still appears in the list.
    let (_, exhausted_token) = store
        .create_invite(&session_id, Role::Operator, 1, None)
        .await
        .unwrap();
    store.consume_invite(&exhausted_token).await.unwrap();

    // Expired invite.
    let past = Utc::now() - Duration::hours(1);
    let (_, _) = store
        .create_invite(&session_id, Role::Viewer, 5, Some(past))
        .await
        .unwrap();

    let rows = store.list_invites_for_session(&session_id).await.unwrap();
    // History view shows everything; filtering is a client concern.
    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn revoke_invite_hard_deletes_row() {
    let (store, session_id) = setup().await;

    let (invite, _) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    store.revoke_invite(&invite.token_sha256).await.unwrap();

    let rows = store.list_invites_for_session(&session_id).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn revoke_invite_twice_is_404() {
    let (store, session_id) = setup().await;

    let (invite, _) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    store.revoke_invite(&invite.token_sha256).await.unwrap();

    // Second call must error — the UI uses this to drop stale rows
    // when two admins revoke concurrently.
    let err = store.revoke_invite(&invite.token_sha256).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn revoked_invite_cannot_be_consumed() {
    let (store, session_id) = setup().await;

    let (invite, raw_token) = store
        .create_invite(&session_id, Role::Operator, 5, None)
        .await
        .unwrap();

    store.revoke_invite(&invite.token_sha256).await.unwrap();

    // A revoked invite must fail the redeem path cleanly — this is
    // the whole point of having a revoke endpoint.
    let result = store.consume_invite(&raw_token).await;
    assert!(result.is_err());
}
