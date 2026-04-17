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
    store.delete_recording_share("hash_abc").await.unwrap();
    let gone = store
        .consume_recording_share("hash_abc", &rec.id)
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

    // Create one with an already-past expiry
    let past = "2020-01-01T00:00:00+00:00";
    store
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

    // Create one with no expiry (permanent)
    store
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

    // Create one with future expiry
    let future = "2099-01-01T00:00:00+00:00";
    store
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

    let expired = store.list_expired_recordings(10).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].file_path, Some("/tmp/old.cast".to_string()));
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
