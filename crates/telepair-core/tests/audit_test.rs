//! Storage-level audit log round-trips.
//!
//! These tests exercise [`Storage::insert_audit_event`] /
//! [`Storage::list_audit_events`] directly against an in-memory
//! SQLite: they are the foundation the higher-level [`AuditSink`] and
//! the CLI sit on, so any regression here would silently break the
//! whole audit feature.

use chrono::{Duration, Utc};
use serde_json::json;
use telepair_core::audit::{AuditEvent, AuditEventType, AuditFilter};
use telepair_core::storage::{SqliteStorage, Storage};
use uuid::Uuid;

async fn store() -> SqliteStorage {
    SqliteStorage::new_memory().await.unwrap()
}

#[tokio::test]
async fn insert_and_query_round_trip() {
    // Minimal happy path: an event with every optional field populated
    // must survive a write-then-read cycle with no data loss. Anything
    // the row helper silently drops would surface here.
    let storage = store().await;
    let actor = Uuid::new_v4();
    let event = AuditEvent::new(AuditEventType::SessionCreated)
        .with_actor(actor, "alice")
        .with_session("sess-123")
        .with_detail(json!({ "target": "local-shell" }));

    let id = storage.insert_audit_event(&event).await.unwrap();
    assert!(id > 0, "autoincrement id should be positive");

    let rows = storage
        .list_audit_events(&AuditFilter::default())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let got = &rows[0];
    assert_eq!(got.id, Some(id));
    assert_eq!(got.actor_id, Some(actor));
    assert_eq!(got.actor_name.as_deref(), Some("alice"));
    assert_eq!(got.event_type, AuditEventType::SessionCreated);
    assert_eq!(got.session_id.as_deref(), Some("sess-123"));
    assert_eq!(got.detail, json!({ "target": "local-shell" }));
}

#[tokio::test]
async fn null_detail_round_trips_as_json_null() {
    // `Value::Null` must land in the DB as SQL NULL — not the literal
    // string "null" — and must read back as `Value::Null`. The UI
    // relies on this symmetry to render "no detail" rows.
    // `TargetAccessDenied` is the one event type that legitimately has
    // no actor and no session (the caller was rejected before either
    // was resolved), so it's the natural fit for exercising the
    // all-NULL path.
    let storage = store().await;
    storage
        .insert_audit_event(&AuditEvent::new(AuditEventType::TargetAccessDenied))
        .await
        .unwrap();

    let rows = storage
        .list_audit_events(&AuditFilter::default())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].detail.is_null());
    assert!(rows[0].actor_id.is_none());
    assert!(rows[0].session_id.is_none());
}

#[tokio::test]
async fn filter_by_actor_and_session() {
    // Multiple rows, mixed shape, three separate filter axes — verify
    // WHERE composition picks up the right rows for each.
    let storage = store().await;
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();

    storage
        .insert_audit_event(
            &AuditEvent::new(AuditEventType::SessionCreated)
                .with_actor(alice, "alice")
                .with_session("sess-a"),
        )
        .await
        .unwrap();
    storage
        .insert_audit_event(
            &AuditEvent::new(AuditEventType::SessionClosed)
                .with_actor(alice, "alice")
                .with_session("sess-a"),
        )
        .await
        .unwrap();
    storage
        .insert_audit_event(
            &AuditEvent::new(AuditEventType::SessionCreated)
                .with_actor(bob, "bob")
                .with_session("sess-b"),
        )
        .await
        .unwrap();

    let by_actor = storage
        .list_audit_events(&AuditFilter {
            actor_id: Some(alice),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_actor.len(), 2);
    assert!(by_actor.iter().all(|e| e.actor_id == Some(alice)));

    let by_session = storage
        .list_audit_events(&AuditFilter {
            session_id: Some("sess-b".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_session.len(), 1);
    assert_eq!(by_session[0].actor_id, Some(bob));
}

#[tokio::test]
async fn filter_by_event_type_in_clause() {
    // The `event_types` filter collapses to `event_type IN (?, ?, ...)`
    // with one bind per element. This test also exercises the
    // "multiple types" branch — a single-type filter would hide
    // an off-by-one in the IN-clause builder.
    let storage = store().await;
    for ty in [
        AuditEventType::SessionCreated,
        AuditEventType::SessionClosed,
        AuditEventType::InviteMinted,
        AuditEventType::InviteRedeemed,
        AuditEventType::TargetAccessDenied,
    ] {
        storage
            .insert_audit_event(&AuditEvent::new(ty))
            .await
            .unwrap();
    }

    let rows = storage
        .list_audit_events(&AuditFilter {
            event_types: vec![AuditEventType::InviteMinted, AuditEventType::InviteRedeemed],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|e| matches!(
        e.event_type,
        AuditEventType::InviteMinted | AuditEventType::InviteRedeemed
    )));
}

#[tokio::test]
async fn filter_by_time_window() {
    // `since` is inclusive, `until` is exclusive. We cannot rely on
    // the DB's `ts` default (there isn't one) so the test builds
    // three events with explicit timestamps and asserts the window
    // catches only the middle row.
    let storage = store().await;
    let now = Utc::now();
    let old = AuditEvent {
        ts: now - Duration::hours(2),
        ..AuditEvent::new(AuditEventType::SessionCreated)
    };
    let mid = AuditEvent {
        ts: now - Duration::minutes(30),
        ..AuditEvent::new(AuditEventType::SessionClosed)
    };
    let recent = AuditEvent {
        ts: now,
        ..AuditEvent::new(AuditEventType::InviteMinted)
    };
    storage.insert_audit_event(&old).await.unwrap();
    storage.insert_audit_event(&mid).await.unwrap();
    storage.insert_audit_event(&recent).await.unwrap();

    let rows = storage
        .list_audit_events(&AuditFilter {
            since: Some(now - Duration::hours(1)),
            until: Some(now),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_type, AuditEventType::SessionClosed);
}

#[tokio::test]
async fn results_are_newest_first_with_limit_and_offset() {
    // Pagination: newest row at index 0, limit caps the page size,
    // and `offset` skips over the freshest rows. The secondary sort
    // on `id DESC` keeps the ordering stable for events inserted
    // within the same millisecond.
    let storage = store().await;
    for _ in 0..5 {
        storage
            .insert_audit_event(&AuditEvent::new(AuditEventType::SessionCreated))
            .await
            .unwrap();
    }

    let page = storage
        .list_audit_events(&AuditFilter {
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    // Newest first means the two highest ids should come back.
    assert!(page[0].id > page[1].id);

    let next = storage
        .list_audit_events(&AuditFilter {
            limit: Some(2),
            offset: 2,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(next.len(), 2);
    // `next` starts where `page` left off — no overlap.
    assert!(next[0].id < page[1].id);
}

#[tokio::test]
async fn default_limit_caps_at_one_hundred() {
    // The empty filter is the CLI's default and the session-detail
    // timeline's default — it must cap at 100 rows so a busy server
    // doesn't dump its entire audit table to anyone who asks.
    let storage = store().await;
    for _ in 0..150 {
        storage
            .insert_audit_event(&AuditEvent::new(AuditEventType::ParticipantJoined))
            .await
            .unwrap();
    }

    let rows = storage
        .list_audit_events(&AuditFilter::default())
        .await
        .unwrap();
    assert_eq!(rows.len(), 100);
}
