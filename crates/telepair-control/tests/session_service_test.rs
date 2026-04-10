use std::sync::Arc;
use telepair_control::session_service::SessionService;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, SessionStatus};
use telepair_core::storage::{SqliteStorage, Storage};

/// Service-level tests live here. The storage handle is returned
/// alongside the service because tests need a fixture to seed users
/// directly — the point of dropping the old `svc.storage()` accessor
/// is that *production code* can no longer reach raw storage through
/// the service, not that tests can't seed fixtures at all.
async fn setup() -> (SessionService, Arc<SqliteStorage>, TokenAuthProvider) {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let svc = SessionService::new(store.clone());
    let auth = TokenAuthProvider::new(store.clone());
    (svc, store, auth)
}

async fn seed_user(
    store: &Arc<SqliteStorage>,
    auth: &TokenAuthProvider,
    name: &str,
) -> telepair_core::session::User {
    let (_, token) = store.create_user(name, false).await.unwrap();
    auth.validate(&token).await.unwrap()
}

#[tokio::test]
async fn create_session_adds_owner_as_participant() {
    let (svc, store, auth) = setup().await;
    let user = seed_user(&store, &auth, "owner").await;
    let session = svc
        .create_session(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();

    let participants = svc.list_participants(&session.id).await.unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].role, Role::Owner);
}

#[tokio::test]
async fn close_session_updates_status() {
    let (svc, store, auth) = setup().await;
    let user = seed_user(&store, &auth, "owner").await;
    let session = svc
        .create_session(user.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    svc.close_session(&session.id).await.unwrap();
    let fetched = svc.get_session_required(&session.id).await.unwrap();
    assert_eq!(fetched.status, SessionStatus::Closed);
}

#[tokio::test]
async fn require_owner_accepts_owner_and_rejects_others() {
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let other = seed_user(&store, &auth, "stranger").await;
    let session = svc
        .create_session(owner.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    // Happy path: owner gets the session back.
    let got = svc.require_owner(&owner, &session.id).await.unwrap();
    assert_eq!(got.id, session.id);

    // Wrong user → PermissionDenied (→ 403 via http_status()).
    let err = svc.require_owner(&other, &session.id).await.unwrap_err();
    assert!(matches!(
        err,
        telepair_core::error::Error::PermissionDenied(_)
    ));

    // Nonexistent session → SessionNotFound (→ 404).
    let err = svc
        .require_owner(&owner, "no-such-session")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        telepair_core::error::Error::SessionNotFound(_)
    ));
}

#[tokio::test]
async fn require_active_owned_rejects_closed_session() {
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let session = svc
        .create_session(owner.id, "shell", InputMode::Serialized)
        .await
        .unwrap();

    // While active, both accessors work.
    svc.require_active_owned(&owner, &session.id).await.unwrap();

    // Close the session — require_owner still returns the row (the
    // plain-ownership accessor doesn't gate on status), but
    // require_active_owned rejects with SessionClosed (→ 410 Gone).
    svc.close_session(&session.id).await.unwrap();
    svc.require_owner(&owner, &session.id).await.unwrap();
    let err = svc
        .require_active_owned(&owner, &session.id)
        .await
        .unwrap_err();
    assert!(matches!(err, telepair_core::error::Error::SessionClosed(_)));
}
