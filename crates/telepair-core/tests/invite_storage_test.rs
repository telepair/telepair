use chrono::{Duration, Utc};
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{CloseReason, InputMode, RedeemIdentity};
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> (SqliteStorage, String) {
    let store = SqliteStorage::new_memory().await.unwrap();
    let (user, _) = store.create_user("host", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "shell", InputMode::Serialized, None)
        .await
        .unwrap();
    (store, session.id)
}

#[tokio::test]
async fn create_and_redeem_invite_token() {
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

    // Redeem once — verifies lookup by raw token works and increments counter
    let outcome = store
        .redeem_invite(&raw_token, RedeemIdentity::NewGuest { name: "g-create1" })
        .await
        .unwrap();
    assert_eq!(outcome.invite.session_id, session_id);
    assert_eq!(outcome.invite.role, Role::Operator);
    assert_eq!(outcome.invite.max_uses, 5);
    assert_eq!(outcome.invite.used_count, 1);
}

#[tokio::test]
async fn redeem_invite_token_burns_last_use() {
    let (store, session_id) = setup().await;

    let (_, raw_token) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    let outcome = store
        .redeem_invite(&raw_token, RedeemIdentity::NewGuest { name: "g-burn1" })
        .await
        .unwrap();
    assert_eq!(outcome.invite.used_count, 1);

    let result = store
        .redeem_invite(&raw_token, RedeemIdentity::NewGuest { name: "g-burn2" })
        .await;
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

    let result = store
        .redeem_invite(&raw_token, RedeemIdentity::NewGuest { name: "g-exp" })
        .await;
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

    // Exhausted invite: create, redeem to burn the single use, assert
    // it still appears in the list so the management dialog can show
    // "fully used" rows without filtering them on the client.
    let (_, exhausted_token) = store
        .create_invite(&session_id, Role::Operator, 1, None)
        .await
        .unwrap();
    store
        .redeem_invite(
            &exhausted_token,
            RedeemIdentity::NewGuest { name: "g-exhaust" },
        )
        .await
        .unwrap();

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
async fn revoke_invite_twice_is_idempotent() {
    // Two admins racing a revoke, or a user retrying after a network
    // timeout, must not get a spurious error on the second call. The
    // storage layer erases the "did the row really exist?" distinction
    // so the HTTP DELETE can answer 204 either way.
    let (store, session_id) = setup().await;

    let (invite, _) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    store.revoke_invite(&invite.token_sha256).await.unwrap();

    // Second call is a no-op — already gone, so nothing to delete.
    store.revoke_invite(&invite.token_sha256).await.unwrap();

    // And an arbitrary unknown sha is also a no-op (not an error).
    store.revoke_invite(&"0".repeat(64)).await.unwrap();
}

#[tokio::test]
async fn revoked_invite_cannot_be_redeemed() {
    let (store, session_id) = setup().await;

    let (invite, raw_token) = store
        .create_invite(&session_id, Role::Operator, 5, None)
        .await
        .unwrap();

    store.revoke_invite(&invite.token_sha256).await.unwrap();

    // A revoked invite must fail the redeem path cleanly — this is
    // the whole point of having a revoke endpoint.
    let result = store
        .redeem_invite(&raw_token, RedeemIdentity::NewGuest { name: "g-rev" })
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn redeem_invite_against_closed_session_is_atomic() {
    // Regression test for the v0.1.1 TOCTOU finding: the invite
    // service used to (1) pre-check "session active?", (2)
    // `consume_invite` (burns used_count), (3) insert participant.
    // A concurrent `close_session` between (1) and (3) would leave
    // a participant row pointing at a closed session AND burn the
    // invite. The atomic `redeem_invite` folds the status check
    // into the UPDATE WHERE and runs the whole sequence in one
    // transaction, so a closed session must produce `SessionClosed`
    // with the invite counter untouched.
    let (store, session_id) = setup().await;
    let (user, _) = store.create_user("joiner", false).await.unwrap();

    let (invite, raw_token) = store
        .create_invite(&session_id, Role::Operator, 3, None)
        .await
        .unwrap();
    assert_eq!(invite.used_count, 0);

    store
        .close_session(&session_id, CloseReason::Owner)
        .await
        .unwrap();

    let result = store
        .redeem_invite(&raw_token, RedeemIdentity::Existing(user.id))
        .await;
    match result {
        Err(Error::SessionClosed(id)) => assert_eq!(id, session_id),
        other => panic!("expected SessionClosed, got {other:?}"),
    }

    // The transaction rolled back cleanly — counter must be
    // untouched, otherwise the invite would have been silently
    // burned for no benefit.
    let rows = store.list_invites_for_session(&session_id).await.unwrap();
    let same = rows
        .iter()
        .find(|row| row.token_sha256 == invite.token_sha256)
        .expect("invite row should still exist");
    assert_eq!(same.used_count, 0);

    // And no participant row was created for the would-be joiner.
    let participants = store.list_participants(&session_id).await.unwrap();
    assert!(
        participants.iter().all(|p| p.user_id != user.id),
        "joiner must not be persisted as participant when session is closed"
    );
}

#[tokio::test]
async fn redeem_invite_existing_member_is_idempotent_at_storage_layer() {
    // Regression for the concurrent double-redeem finding:
    // `InviteService::redeem` does a service-layer
    // `find_active_participant_role` pre-check before calling
    // `storage.redeem_invite`. Two concurrent redeems from the
    // same authenticated user could each observe "not a member"
    // and each reach the storage transaction; the pre-fix UPDATE
    // bumped `used_count` unconditionally and the participant
    // upsert collapsed both into a single row, silently burning
    // one or more extra uses on a multi-use invite.
    //
    // The fix adds a `NOT EXISTS(active participant for same user)`
    // clause to the Existing-identity UPDATE and, when that fires,
    // returns an idempotent `was_already_member: true` outcome
    // without bumping `used_count`. This test drives the storage
    // layer directly (bypassing the service pre-check) to pin the
    // invariant: the race-loser path is a no-op at the storage
    // boundary.
    let (store, session_id) = setup().await;
    let (alice, _) = store.create_user("alice", false).await.unwrap();

    let (_, raw_token) = store
        .create_invite(&session_id, Role::Operator, 5, None)
        .await
        .unwrap();

    // First redeem: Alice joins fresh, `used_count` 0 → 1.
    let first = store
        .redeem_invite(&raw_token, RedeemIdentity::Existing(alice.id))
        .await
        .unwrap();
    assert_eq!(first.invite.used_count, 1);
    assert!(
        !first.was_already_member,
        "fresh join must NOT be flagged as already-member"
    );

    // Second redeem for the SAME user — simulates the race-loser
    // path. Must flip `was_already_member` and must NOT bump
    // `used_count` again.
    let second = store
        .redeem_invite(&raw_token, RedeemIdentity::Existing(alice.id))
        .await
        .unwrap();
    assert!(
        second.was_already_member,
        "second redeem by same user must take the idempotent short path"
    );

    // Re-read the invite row via a separate call so the assertion
    // is against authoritative storage state, not a possibly-cached
    // field on `second.invite`.
    let post = store.find_invite(&raw_token).await.unwrap();
    assert_eq!(
        post.used_count, 1,
        "second redeem must not burn a second use"
    );

    // And the participants table carries exactly one row for Alice
    // — the upsert collapsing was already covered, this is the
    // complementary assertion that proves `used_count` and
    // participant count stay in lockstep.
    let parts = store.list_participants(&session_id).await.unwrap();
    let alice_count = parts.iter().filter(|p| p.user_id == alice.id).count();
    assert_eq!(
        alice_count, 1,
        "Alice must appear in participants exactly once"
    );
}

#[tokio::test]
async fn redeem_invite_new_guest_rolls_back_on_closed_session() {
    // Same guarantee as `redeem_invite_against_closed_session_is_atomic`,
    // but exercising the NewGuest identity path — a closed session
    // must not leak a freshly-minted guest `users` row either.
    let (store, session_id) = setup().await;

    let (_, raw_token) = store
        .create_invite(&session_id, Role::Viewer, 1, None)
        .await
        .unwrap();

    store
        .close_session(&session_id, CloseReason::Owner)
        .await
        .unwrap();

    // Use a fixed, distinctive name so we can probe by name rather
    // than counting rows — the test must prove *that specific guest*
    // was not persisted on the failing path.
    let guest_name = "guest-redeemX";
    let result = store
        .redeem_invite(&raw_token, RedeemIdentity::NewGuest { name: guest_name })
        .await;
    assert!(matches!(result, Err(Error::SessionClosed(_))));

    // The rollback must have dropped the INSERT — otherwise a
    // closed-session redeem would slowly accumulate ghost guests.
    let looked_up = store.get_user_by_name(guest_name).await.unwrap();
    assert!(
        looked_up.is_none(),
        "failed redeem must not leak a users row: {looked_up:?}"
    );
}
