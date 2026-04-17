use std::sync::Arc;
use telepair_control::session_service::SessionService;
use telepair_core::audit::AuditSink;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::permission::Role;
use telepair_core::session::{CloseReason, InputMode, SessionListFilter, SessionStatus};
use telepair_core::storage::{SqliteStorage, Storage};

/// Service-level tests live here. The storage handle is returned
/// alongside the service because tests need a fixture to seed users
/// directly — the point of dropping the old `svc.storage()` accessor
/// is that *production code* can no longer reach raw storage through
/// the service, not that tests can't seed fixtures at all.
async fn setup() -> (SessionService, Arc<SqliteStorage>, TokenAuthProvider) {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let audit = Arc::new(AuditSink::new(store.clone()));
    let svc = SessionService::new(store.clone(), audit);
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
        .create_session(&user, "local-shell", InputMode::Serialized)
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
        .create_session(&user, "shell", InputMode::Serialized)
        .await
        .unwrap();

    svc.close_session(&session.id, CloseReason::Owner, Some(&user))
        .await
        .unwrap();
    let fetched = svc.get_session_required(&session.id).await.unwrap();
    assert_eq!(fetched.status, SessionStatus::Closed);
    assert_eq!(fetched.closed_reason, Some(CloseReason::Owner));
}

#[tokio::test]
async fn require_owner_accepts_owner_and_rejects_others() {
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let other = seed_user(&store, &auth, "stranger").await;
    let session = svc
        .create_session(&owner, "shell", InputMode::Serialized)
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
        .create_session(&owner, "shell", InputMode::Serialized)
        .await
        .unwrap();

    // While active, both accessors work.
    svc.require_active_owned(&owner, &session.id).await.unwrap();

    // Close the session — require_owner still returns the row (the
    // plain-ownership accessor doesn't gate on status), but
    // require_active_owned rejects with SessionClosed (→ 410 Gone).
    svc.close_session(&session.id, CloseReason::Owner, Some(&owner))
        .await
        .unwrap();
    svc.require_owner(&owner, &session.id).await.unwrap();
    let err = svc
        .require_active_owned(&owner, &session.id)
        .await
        .unwrap_err();
    assert!(matches!(err, telepair_core::error::Error::SessionClosed(_)));
}

#[tokio::test]
async fn list_sessions_for_user_history_includes_closed_rows() {
    // The history view wants to show closed sessions the user
    // owned, not just the ones they're currently in. Default filter
    // (no status) must surface them; `active_only()` must hide them.
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;

    // Active session — should show up on both filters.
    let alive = svc
        .create_session(&owner, "alive-target", InputMode::Serialized)
        .await
        .unwrap();

    // Closed session — should show up on the default filter AND on
    // `Some(Closed)`, but not on `active_only()`. Reaper path: no
    // actor, matching the production call site in session_hub.
    let dead = svc
        .create_session(&owner, "dead-target", InputMode::Serialized)
        .await
        .unwrap();
    svc.close_session(&dead.id, CloseReason::Reaper, None)
        .await
        .unwrap();

    // Default = both statuses, newest first.
    let all = svc
        .list_sessions_for_user(owner.id, SessionListFilter::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let ids: Vec<_> = all.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&alive.id.as_str()));
    assert!(ids.contains(&dead.id.as_str()));

    // Active-only should return just the alive row.
    let active = svc
        .list_sessions_for_user(owner.id, SessionListFilter::active_only())
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, alive.id);

    // Closed filter should return just the dead row, with the
    // reaper-stamped reason preserved.
    let closed = svc
        .list_sessions_for_user(
            owner.id,
            SessionListFilter {
                status: Some(SessionStatus::Closed),
                ..SessionListFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id, dead.id);
    assert_eq!(closed[0].closed_reason, Some(CloseReason::Reaper));
}

#[tokio::test]
async fn list_sessions_for_user_target_filter_narrows_results() {
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;

    let shell = svc
        .create_session(&owner, "shell", InputMode::Serialized)
        .await
        .unwrap();
    svc.create_session(&owner, "other", InputMode::Serialized)
        .await
        .unwrap();

    let shell_only = svc
        .list_sessions_for_user(
            owner.id,
            SessionListFilter {
                target_name: Some("shell".into()),
                ..SessionListFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(shell_only.len(), 1);
    assert_eq!(shell_only[0].id, shell.id);
}

#[tokio::test]
async fn list_sessions_visible_to_admin_sees_every_session() {
    // The Dashboard Sessions tab and the admin targets deep-link
    // both need admins to see every session in the system, not just
    // the ones the admin happens to own or have joined. Without
    // this, the "N active sessions" count on the admin targets card
    // and the list shown after the deep-link would silently diverge
    // — an operator could click a card showing "5 active" and see
    // an empty list because none of those sessions belonged to them.
    let (svc, store, auth) = setup().await;
    let alice = seed_user(&store, &auth, "alice").await;
    let bob = seed_user(&store, &auth, "bob").await;
    let (_, admin_token) = store.create_user("root", true).await.unwrap();
    let admin = auth.validate(&admin_token).await.unwrap();

    // Alice owns two sessions; Bob owns one. The admin owns nothing.
    svc.create_session(&alice, "shell", InputMode::Serialized)
        .await
        .unwrap();
    svc.create_session(&alice, "shell", InputMode::Serialized)
        .await
        .unwrap();
    svc.create_session(&bob, "shell", InputMode::Serialized)
        .await
        .unwrap();

    // Admin visibility: all three rows, regardless of ownership.
    let admin_view = svc
        .list_sessions_visible_to(&admin, SessionListFilter::default())
        .await
        .unwrap();
    assert_eq!(
        admin_view.len(),
        3,
        "admins must see every session in the system"
    );

    // Non-admin visibility: only the rows they actually own or
    // participate in. Alice sees her two, Bob sees his one.
    let alice_view = svc
        .list_sessions_visible_to(&alice, SessionListFilter::default())
        .await
        .unwrap();
    assert_eq!(alice_view.len(), 2);
    assert!(alice_view.iter().all(|s| s.owner_id == alice.id));

    let bob_view = svc
        .list_sessions_visible_to(&bob, SessionListFilter::default())
        .await
        .unwrap();
    assert_eq!(bob_view.len(), 1);
    assert_eq!(bob_view[0].owner_id, bob.id);
}

#[tokio::test]
async fn list_sessions_visible_to_admin_honours_filter() {
    // The filter (target_name / status / limit) must still apply
    // under the admin branch — admin privilege widens the visible
    // set but does not disable query predicates.
    let (svc, store, auth) = setup().await;
    let alice = seed_user(&store, &auth, "alice").await;
    let (_, admin_token) = store.create_user("root", true).await.unwrap();
    let admin = auth.validate(&admin_token).await.unwrap();

    svc.create_session(&alice, "shell", InputMode::Serialized)
        .await
        .unwrap();
    svc.create_session(&alice, "other", InputMode::Serialized)
        .await
        .unwrap();

    let shell_only = svc
        .list_sessions_visible_to(
            &admin,
            SessionListFilter {
                target_name: Some("shell".into()),
                ..SessionListFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(shell_only.len(), 1);
    assert_eq!(shell_only[0].target_name, "shell");
}

#[tokio::test]
async fn close_session_by_user_happy_path_marks_session_closed() {
    // The HTTP `DELETE /api/sessions/:id` handler used to inline a
    // `get_session + owner check + close_session` triple, which made
    // it possible for a future contributor to add a code path that
    // bypassed the owner check (or stamped a different CloseReason).
    // The wrapper consolidates the rule. Verify the happy path:
    // owner → session.status flips to Closed with reason Owner.
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let session = svc
        .create_session(&owner, "shell", InputMode::Serialized)
        .await
        .unwrap();

    svc.close_session_by_user(&owner, &session.id)
        .await
        .unwrap();

    let after = svc.get_session_required(&session.id).await.unwrap();
    assert_eq!(after.status, SessionStatus::Closed);
    assert_eq!(after.closed_reason, Some(CloseReason::Owner));
}

#[tokio::test]
async fn close_session_by_user_rejects_non_owner_non_admin_and_missing() {
    // Both error variants must round-trip to the same HTTP statuses
    // the inline check produced (404 / 403). The gateway lifts these
    // through `Error::http_status` and the assertions below pin
    // *that* mapping by matching the variants directly — that way a
    // future change to `http_status()` cannot silently turn the 403
    // into a generic 500.
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let stranger = seed_user(&store, &auth, "stranger").await;
    let session = svc
        .create_session(&owner, "shell", InputMode::Serialized)
        .await
        .unwrap();

    // Wrong user (non-admin) → PermissionDenied → 403, and the
    // session must NOT have been closed as a side effect.
    let err = svc
        .close_session_by_user(&stranger, &session.id)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        telepair_core::error::Error::PermissionDenied(_)
    ));
    let still_alive = svc.get_session_required(&session.id).await.unwrap();
    assert_eq!(still_alive.status, SessionStatus::Active);

    // Nonexistent session → SessionNotFound → 404.
    let err = svc
        .close_session_by_user(&owner, "no-such-session")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        telepair_core::error::Error::SessionNotFound(_)
    ));
}

#[tokio::test]
async fn close_session_by_user_is_idempotent_on_already_closed() {
    // Regression for F3: the UI's Close button is debounced but a
    // double-click within the ack window used to surface a confusing
    // "session not found" toast because the storage-layer UPDATE
    // filters on `status = 'active'` and a second call matched zero
    // rows. Verify that the second close returns Ok and the session
    // stays in its post-first-close state.
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let session = svc
        .create_session(&owner, "shell", InputMode::Serialized)
        .await
        .unwrap();

    svc.close_session_by_user(&owner, &session.id)
        .await
        .unwrap();
    // Second call is a no-op — not a 404. Anything else would mean
    // the UI has to special-case "already closed" on every retry.
    svc.close_session_by_user(&owner, &session.id)
        .await
        .unwrap();

    let after = svc.get_session_required(&session.id).await.unwrap();
    assert_eq!(after.status, SessionStatus::Closed);
    // Reason must still be the original `Owner`, not overwritten by
    // the second (no-op) call.
    assert_eq!(after.closed_reason, Some(CloseReason::Owner));
}

#[tokio::test]
async fn close_session_by_user_lets_admin_force_close_foreign_session() {
    // Regression for F4: after an admin disables a user, that user's
    // still-active session used to sit around until the idle reaper
    // hit — `DELETE /api/sessions/:id` returned 403 because the
    // admin wasn't the owner. Admins must be able to clean up.
    // The close reason stamps as `Admin`, not `Owner`, so the
    // history view doesn't misattribute the action.
    let (svc, store, auth) = setup().await;
    let owner = seed_user(&store, &auth, "owner").await;
    let mut admin = seed_user(&store, &auth, "admin").await;
    admin.is_admin = true;
    let session = svc
        .create_session(&owner, "shell", InputMode::Serialized)
        .await
        .unwrap();

    svc.close_session_by_user(&admin, &session.id)
        .await
        .unwrap();

    let after = svc.get_session_required(&session.id).await.unwrap();
    assert_eq!(after.status, SessionStatus::Closed);
    assert_eq!(after.closed_reason, Some(CloseReason::Admin));
}
