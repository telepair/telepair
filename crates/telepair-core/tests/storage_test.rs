use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use telepair_core::permission::Role;
use telepair_core::session::{CloseReason, InputMode, SessionListFilter};
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
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
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
        .create_session_with_owner(owner.id, "shell", InputMode::Serialized, None)
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
        .create_session_with_owner(user.id, "shell", InputMode::Serialized, None)
        .await
        .unwrap();

    store
        .close_session(&session.id, CloseReason::Owner)
        .await
        .unwrap();
    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(
        fetched.status,
        telepair_core::session::SessionStatus::Closed
    );
    assert!(fetched.closed_at.is_some());
    // The reason the caller passed must round-trip — the history
    // view reads this to pick the right chip.
    assert_eq!(fetched.closed_reason, Some(CloseReason::Owner));
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
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
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
        .create_session_with_owner(ghost_owner, "local-shell", InputMode::Serialized, None)
        .await;
    assert!(
        result.is_err(),
        "creating a session for a nonexistent owner must fail"
    );

    // Nothing should be hanging around from the rolled-back tx. Query
    // by the ghost owner — if the FK check rolled back properly, no
    // sessions should be associated with that owner id.
    let ghost_sessions = store
        .list_sessions_for_user(ghost_owner, SessionListFilter::default())
        .await
        .unwrap();
    assert!(
        ghost_sessions.is_empty(),
        "rolled-back transaction must not leave session rows: {ghost_sessions:?}"
    );
}

#[tokio::test]
async fn list_sessions_offset_without_limit_returns_remaining_rows() {
    // Regression for the v0.1.1-dev bug where `SessionListFilter`
    // carrying `offset > 0` but no `limit` produced `... ORDER BY ...
    // OFFSET ?` — SQLite rejects that as a syntax error and the
    // request 500s. The fix emits `LIMIT -1` when offset is set
    // without an explicit cap.
    let store = setup().await;
    let (user, _) = store.create_user("paginator", false).await.unwrap();
    for _ in 0..3 {
        store
            .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
            .await
            .unwrap();
    }

    // offset=1, no limit → should return 2 rows, NOT error out.
    let filter = SessionListFilter {
        offset: 1,
        ..SessionListFilter::default()
    };
    let rows = store.list_sessions_for_user(user.id, filter).await.unwrap();
    assert_eq!(rows.len(), 2, "offset=1 of 3 rows should yield 2");

    // offset=0 with no limit still works (pre-existing path — guard
    // against the fix over-reaching).
    let rows_all = store
        .list_sessions_for_user(user.id, SessionListFilter::default())
        .await
        .unwrap();
    assert_eq!(rows_all.len(), 3);
}

#[tokio::test]
async fn list_sessions_target_name_filter_narrows_results() {
    // The Dashboard admin-link deep-filters by target_name; make sure
    // the storage layer honors it. Seeds two sessions on different
    // targets for the same user, filters down to one, and asserts
    // the other is excluded.
    let store = setup().await;
    let (user, _) = store.create_user("deeplink", false).await.unwrap();
    let kept = store
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    store
        .create_session_with_owner(user.id, "other-target", InputMode::Serialized, None)
        .await
        .unwrap();

    let filter = SessionListFilter {
        target_name: Some("local-shell".to_string()),
        ..SessionListFilter::default()
    };
    let rows = store.list_sessions_for_user(user.id, filter).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, kept.id);
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
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
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

#[tokio::test]
async fn deleting_session_cascades_to_participants_and_invites() {
    // The participants and invite_tokens FKs to sessions(id) declare
    // ON DELETE CASCADE so admin tooling that hard-deletes a session
    // row sweeps the dependent rows in one statement instead of
    // leaving FK violations or orphan records. SQLite enforces
    // CASCADE only when `PRAGMA foreign_keys=ON`, which SqliteStorage
    // sets via SqliteConnectOptions::foreign_keys(true) — so this
    // test pins both the schema and the connection wiring.
    let uri = "file:cascade_delete_test?mode=memory&cache=shared".to_string();
    // The seed pool needs foreign_keys=ON too, otherwise the manual
    // DELETE we issue would NOT trigger the cascade and the test
    // would silently pass for the wrong reason.
    let options: SqliteConnectOptions = uri.parse().unwrap();
    let options = options.foreign_keys(true);
    let seed_pool = SqlitePool::connect_with(options).await.unwrap();

    let store = SqliteStorage::new(&uri).await.unwrap();
    let (user, _) = store.create_user("doomed", false).await.unwrap();
    let session = store
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    let (_, _raw) = store
        .create_invite(&session.id, Role::Operator, 3, None)
        .await
        .unwrap();

    // Sanity: participant + invite both exist before the delete.
    let participants_before = store.list_participants(&session.id).await.unwrap();
    assert_eq!(participants_before.len(), 1);
    let invite_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM invite_tokens WHERE session_id = ?")
            .bind(&session.id)
            .fetch_one(&seed_pool)
            .await
            .unwrap();
    assert_eq!(invite_count_before, 1);

    // Hard-delete the session row directly.
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(&session.id)
        .execute(&seed_pool)
        .await
        .unwrap();

    let participant_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM participants WHERE session_id = ?")
            .bind(&session.id)
            .fetch_one(&seed_pool)
            .await
            .unwrap();
    assert_eq!(
        participant_count_after, 0,
        "participants must cascade-delete when their session is removed"
    );
    let invite_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM invite_tokens WHERE session_id = ?")
            .bind(&session.id)
            .fetch_one(&seed_pool)
            .await
            .unwrap();
    assert_eq!(
        invite_count_after, 0,
        "invite_tokens must cascade-delete when their session is removed"
    );

    drop(seed_pool);
}
