//! Integration tests for `GET /api/admin/audit/export`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::audit::{AuditEvent, AuditEventType, AuditSink};
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};

async fn setup() -> (axum::Router, String) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (admin, admin_token) = storage.create_user("admin", true).await.unwrap();

    // Seed audit events
    let audit = AuditSink::new(storage.clone());
    audit
        .record(
            AuditEvent::new(AuditEventType::SessionCreated)
                .with_actor(admin.id, "admin".to_string())
                .with_session("sess-1".to_string()),
        )
        .await;
    audit
        .record(
            AuditEvent::new(AuditEventType::SessionClosed)
                .with_actor(admin.id, "admin".to_string())
                .with_session("sess-1".to_string()),
        )
        .await;

    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();
    (router, admin_token)
}

#[tokio::test]
async fn export_json_returns_all_rows() {
    let (app, admin_token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/audit/export?format=json")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("application/json"));
    assert!(resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("attachment"));

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let events: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(events.len() >= 2);
}

#[tokio::test]
async fn export_csv_returns_valid_csv() {
    let (app, admin_token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/audit/export?format=csv")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/csv"));

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    // First line is header
    assert!(text.starts_with("id,timestamp,event_type,actor_id,actor_name,session_id,detail"));
    // At least header + 2 data rows
    let line_count = text.lines().count();
    assert!(line_count >= 3, "expected >=3 lines, got {line_count}");

    // String fields (actor_name, session_id) should be quoted per RFC 4180
    let data_line = text.lines().nth(1).unwrap();
    // actor_name "admin" is quoted → "admin" appears in the line
    assert!(data_line.contains("\"admin\""), "actor_name should be RFC 4180 quoted: {data_line}");
    // session_id "sess-1" is quoted
    assert!(data_line.contains("\"sess-1\""), "session_id should be RFC 4180 quoted: {data_line}");
}

#[tokio::test]
async fn export_missing_format_is_400() {
    let (app, admin_token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/audit/export")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn export_invalid_format_is_400() {
    let (app, admin_token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/audit/export?format=xml")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn export_csv_neutralizes_formula_prefixes() {
    // An attacker-controlled actor_name starting with `=` would be
    // evaluated as a formula by Excel / Sheets / Numbers, even when
    // RFC 4180-quoted. The export must guard every string cell.
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (admin, admin_token) = storage.create_user("admin", true).await.unwrap();
    let audit = AuditSink::new(storage.clone());

    // Plant hostile content: formula-prefixed actor_name AND a
    // session_id with a tab-prefix; detail carries a `=` inside JSON.
    audit
        .record(
            AuditEvent::new(AuditEventType::SessionCreated)
                .with_actor(admin.id, "=cmd|'/c calc'!A1".to_string())
                .with_session("\tleaky".to_string())
                .with_detail(serde_json::json!({"note": "=HYPERLINK(\"http://evil\")"})),
        )
        .await;

    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    let resp = app
        .oneshot(
            Request::get("/api/admin/audit/export?format=csv")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();

    // No raw cell may start with a formula trigger character (after the
    // opening quote). Check every non-header line and every field.
    for line in text.lines().skip(1) {
        // Split carefully: we only care about quoted cells. Look for
        // the pattern `,"` then inspect the first char after it.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'"' && (i == 0 || bytes[i - 1] == b',') {
                let first = bytes.get(i + 1).copied();
                if let Some(c) = first {
                    assert!(
                        !matches!(c, b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r'),
                        "CSV cell starts with formula trigger: {line}"
                    );
                }
                // Skip to next unescaped closing quote
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        if bytes.get(i + 1) == Some(&b'"') {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    // Guarded cells should retain their content but be prefixed by a
    // single quote — verify the original payload is still readable.
    assert!(
        text.contains("'=cmd"),
        "guarded actor_name content lost: {text}"
    );
    assert!(
        text.contains("'\tleaky"),
        "guarded session_id content lost: {text}"
    );
}

#[tokio::test]
async fn export_requires_admin() {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, user_token) = storage.create_user("user", false).await.unwrap();
    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    let resp = app
        .oneshot(
            Request::get("/api/admin/audit/export?format=json")
                .header("Authorization", format!("Bearer {user_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
