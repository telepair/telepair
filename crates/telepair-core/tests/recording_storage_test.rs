use telepair_core::recording::RecordingStatus;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> SqliteStorage {
    SqliteStorage::new_memory().await.unwrap()
}

/// Create a user + session, returning (user_id, session_id).
async fn seed(store: &SqliteStorage) -> (uuid::Uuid, String) {
    let (user, _) = store.create_user("recorder", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "default", InputMode::Serialized, None)
        .await
        .unwrap();
    (user.id, session.id)
}

#[tokio::test]
async fn recording_storage_create_and_get_recording() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let rec = store
        .create_recording(
            "rec_test",
            &session_id,
            user_id,
            120,
            40,
            "/tmp/test.cast",
            None,
        )
        .await
        .unwrap();

    assert_eq!(rec.session_id, session_id);
    assert_eq!(rec.status, "recording");
    assert_eq!(rec.width, 120);
    assert_eq!(rec.height, 40);
    assert_eq!(rec.file_path, Some("/tmp/test.cast".to_string()));
    assert_eq!(rec.created_by, user_id.to_string());
    assert!(rec.expires_at.is_none());

    let fetched = store.get_recording(&rec.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, rec.id);
    assert_eq!(fetched.session_id, session_id);
    assert_eq!(fetched.width, 120);
    assert_eq!(fetched.height, 40);
}

#[tokio::test]
async fn recording_storage_complete_recording() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let rec = store
        .create_recording("rec_c", &session_id, user_id, 80, 24, "/tmp/c.cast", None)
        .await
        .unwrap();

    store
        .complete_recording(&rec.id, 5000, 42, 8192)
        .await
        .unwrap();

    let fetched = store.get_recording(&rec.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, RecordingStatus::Completed.as_str());
    assert_eq!(fetched.duration_ms, Some(5000));
    assert_eq!(fetched.event_count, 42);
    assert_eq!(fetched.file_size, 8192);
    assert!(fetched.completed_at.is_some());
}

#[tokio::test]
async fn recording_storage_fail_recording() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let rec = store
        .create_recording("rec_f", &session_id, user_id, 80, 24, "/tmp/f.cast", None)
        .await
        .unwrap();

    store.fail_recording(&rec.id).await.unwrap();

    let fetched = store.get_recording(&rec.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, RecordingStatus::Failed.as_str());
    assert!(fetched.completed_at.is_some());
}

#[tokio::test]
async fn recording_storage_list_recordings_for_user() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    // Create a second user with their own recording
    let (other, _) = store.create_user("other", false).await.unwrap();
    let other_session = store
        .create_session_with_owner(other.id, "default", InputMode::Serialized, None)
        .await
        .unwrap();

    store
        .create_recording("rec_a", &session_id, user_id, 80, 24, "/tmp/a.cast", None)
        .await
        .unwrap();
    store
        .create_recording("rec_b", &session_id, user_id, 80, 24, "/tmp/b.cast", None)
        .await
        .unwrap();
    store
        .create_recording(
            "rec_c2",
            &other_session.id,
            other.id,
            80,
            24,
            "/tmp/c.cast",
            None,
        )
        .await
        .unwrap();

    let mine = store.list_recordings_for_user(user_id).await.unwrap();
    assert_eq!(mine.len(), 2);
    for r in &mine {
        assert_eq!(r.created_by, user_id.to_string());
    }

    let theirs = store.list_recordings_for_user(other.id).await.unwrap();
    assert_eq!(theirs.len(), 1);
}

#[tokio::test]
async fn recording_storage_find_active_recording_for_session() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    // No active recording yet
    let none = store.find_active_recording(&session_id).await.unwrap();
    assert!(none.is_none());

    // Create one
    let rec = store
        .create_recording(
            "rec_active",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/active.cast",
            None,
        )
        .await
        .unwrap();

    let active = store
        .find_active_recording(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.id, rec.id);

    // Complete it — should no longer be found
    store
        .complete_recording(&rec.id, 1000, 10, 512)
        .await
        .unwrap();
    let gone = store.find_active_recording(&session_id).await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn recording_storage_share_crud() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let rec = store
        .create_recording(
            "rec_share",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/share.cast",
            None,
        )
        .await
        .unwrap();

    // Create a share
    let share = store
        .create_recording_share(&rec.id, "hash_abc", 5, None)
        .await
        .unwrap();
    assert_eq!(share.recording_id, rec.id);
    assert_eq!(share.max_uses, 5);
    assert_eq!(share.used_count, 0);

    // List shares
    let shares = store.list_recording_shares(&rec.id).await.unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].token_sha256, "hash_abc");

    // Consume increments and returns the updated row.
    let consumed = store
        .consume_recording_share("hash_abc", &rec.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed.recording_id, rec.id);
    assert_eq!(consumed.used_count, 1);

    // Wrong recording id returns None and does not increment.
    let mismatch = store
        .consume_recording_share("hash_abc", "other-rec")
        .await
        .unwrap();
    assert!(mismatch.is_none());

    // Delete
    let deleted = store
        .delete_recording_share(&rec.id, "hash_abc")
        .await
        .unwrap();
    assert!(deleted, "delete must report a row was removed");
    let gone = store
        .consume_recording_share("hash_abc", &rec.id)
        .await
        .unwrap();
    assert!(gone.is_none());
}

/// Regression for the cross-recording revoke bug: before the fix,
/// `delete_recording_share` took only the digest and happily removed
/// shares belonging to *any* recording, so any authenticated owner
/// could revoke another owner's share by hitting the endpoint with
/// their own `recording_id`. The scoped delete now returns `false`
/// (→ 404 at the HTTP layer) for a mismatched `recording_id` and
/// leaves the share intact.
#[tokio::test]
async fn recording_storage_delete_share_is_scoped_to_recording_id() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    // Two recordings so we can try to revoke one's share using the
    // other's id.
    let rec_a = store
        .create_recording(
            "rec_scope_a",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/scope_a.cast",
            None,
        )
        .await
        .unwrap();
    let rec_b = store
        .create_recording(
            "rec_scope_b",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/scope_b.cast",
            None,
        )
        .await
        .unwrap();

    store
        .create_recording_share(&rec_a.id, "hash_victim", 0, None)
        .await
        .unwrap();

    // Mismatched recording id must NOT remove anything.
    let deleted = store
        .delete_recording_share(&rec_b.id, "hash_victim")
        .await
        .unwrap();
    assert!(
        !deleted,
        "revoke with mismatched recording_id must be a no-op"
    );

    // The victim share is still usable.
    let still_valid = store
        .consume_recording_share("hash_victim", &rec_a.id)
        .await
        .unwrap();
    assert!(
        still_valid.is_some(),
        "share on rec_a must survive a cross-recording revoke attempt"
    );

    // Correctly scoped revoke DOES remove the share.
    let deleted = store
        .delete_recording_share(&rec_a.id, "hash_victim")
        .await
        .unwrap();
    assert!(deleted);
    let gone = store
        .consume_recording_share("hash_victim", &rec_a.id)
        .await
        .unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn recording_storage_consume_share_blocks_when_exhausted_or_expired() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let rec = store
        .create_recording(
            "rec_consume",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/consume.cast",
            None,
        )
        .await
        .unwrap();

    // max_uses = 1 share — second consume must miss.
    store
        .create_recording_share(&rec.id, "hash_one_use", 1, None)
        .await
        .unwrap();
    let first = store
        .consume_recording_share("hash_one_use", &rec.id)
        .await
        .unwrap();
    assert!(first.is_some(), "first use must succeed");
    let second = store
        .consume_recording_share("hash_one_use", &rec.id)
        .await
        .unwrap();
    assert!(second.is_none(), "second use must be blocked");

    // Expired share — consume must miss without incrementing.
    let past = "2020-01-01T00:00:00+00:00";
    store
        .create_recording_share(&rec.id, "hash_expired", 0, Some(past))
        .await
        .unwrap();
    let expired = store
        .consume_recording_share("hash_expired", &rec.id)
        .await
        .unwrap();
    assert!(expired.is_none(), "expired share must be blocked");
}

#[tokio::test]
async fn recording_storage_list_expired_recordings() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    // Create one with an already-past expiry and mark it completed —
    // the TTL cleaner only considers finished rows now, so leaving it
    // at the default `status = 'recording'` would filter it out.
    let past = "2020-01-01T00:00:00+00:00";
    let old = store
        .create_recording(
            "rec_old",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/old.cast",
            Some(past),
        )
        .await
        .unwrap();
    store
        .complete_recording(&old.id, 1000, 5, 512)
        .await
        .unwrap();

    // Create one with no expiry (permanent)
    let perm = store
        .create_recording(
            "rec_perm_e",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/perm.cast",
            None,
        )
        .await
        .unwrap();
    store
        .complete_recording(&perm.id, 1000, 5, 512)
        .await
        .unwrap();

    // Create one with future expiry
    let future = "2099-01-01T00:00:00+00:00";
    let fut = store
        .create_recording(
            "rec_future",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/future.cast",
            Some(future),
        )
        .await
        .unwrap();
    store
        .complete_recording(&fut.id, 1000, 5, 512)
        .await
        .unwrap();

    let expired = store.list_expired_recordings(10).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].file_path, Some("/tmp/old.cast".to_string()));
}

/// Defense-in-depth for "the TTL cleaner must never try to delete a
/// recording that is still being captured." Even if a bad
/// `expires_at` write or a wall-clock jump makes an active
/// recording's expiry look past, `list_expired_recordings` filters
/// `status != 'recording'` so the cleaner doesn't hand that row to
/// `delete_recording` and rip the writer's file out from under it.
#[tokio::test]
async fn recording_storage_list_expired_skips_active_recordings() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    // Simulate a pathological row: status='recording' with an
    // already-past `expires_at`. Only the direct SQL update lets us
    // construct this — the service layer would never mint such a
    // row — so this is purely about making sure the filter catches
    // it if it somehow exists.
    let past = "2020-01-01T00:00:00+00:00";
    store
        .create_recording(
            "rec_active_past",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/active_past.cast",
            Some(past),
        )
        .await
        .unwrap();

    let expired = store.list_expired_recordings(10).await.unwrap();
    assert!(
        expired.iter().all(|r| r.status != "recording"),
        "active recordings must never appear in the expired list; got {expired:?}",
    );
}

#[tokio::test]
async fn recording_storage_delete_recording() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let rec = store
        .create_recording(
            "rec_del",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/del.cast",
            None,
        )
        .await
        .unwrap();

    // Add a share to verify cascade
    store
        .create_recording_share(&rec.id, "hash_del", 3, None)
        .await
        .unwrap();

    store.delete_recording(&rec.id).await.unwrap();

    let gone = store.get_recording(&rec.id).await.unwrap();
    assert!(gone.is_none());

    // Share should also be gone (cascade)
    let shares = store.list_recording_shares(&rec.id).await.unwrap();
    assert!(shares.is_empty());
}

#[tokio::test]
async fn recording_storage_set_recording_permanent() {
    let store = setup().await;
    let (user_id, session_id) = seed(&store).await;

    let expiry = "2099-01-01T00:00:00+00:00";
    let rec = store
        .create_recording(
            "rec_perm",
            &session_id,
            user_id,
            80,
            24,
            "/tmp/perm.cast",
            Some(expiry),
        )
        .await
        .unwrap();
    assert!(rec.expires_at.is_some());

    // Make permanent (clear expires_at)
    store.set_recording_permanent(&rec.id).await.unwrap();
    let fetched = store.get_recording(&rec.id).await.unwrap().unwrap();
    assert!(fetched.expires_at.is_none());

    // Restore an expiry
    let new_expiry = "2098-06-15T00:00:00+00:00";
    store
        .set_recording_expiry(&rec.id, new_expiry)
        .await
        .unwrap();
    let fetched2 = store.get_recording(&rec.id).await.unwrap().unwrap();
    assert_eq!(fetched2.expires_at, Some(new_expiry.to_string()));
}
