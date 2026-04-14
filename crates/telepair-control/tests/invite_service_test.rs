//! Unit tests for [`telepair_control::invite_service::InviteService`].
//!
//! These cover the business-rule surface that used to live inline in
//! the gateway's `http::create_invite` / `http::redeem_invite`
//! handlers. The HTTP layer now hands off to the service, so the
//! behavioral contract lives here instead of in gateway integration
//! tests — one happy-path + one boundary test per rule.

use std::sync::Arc;

use chrono::{Duration, Utc};

use telepair_control::invite_service::{
    CreateInviteParams, InviteService, MAX_INVITE_TTL_MINUTES, MAX_INVITE_USES,
};
use telepair_control::session_service::SessionService;
use telepair_core::audit::AuditSink;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{CloseReason, InputMode, Session, User};
use telepair_core::storage::{SqliteStorage, Storage};

struct Fixture {
    invites: InviteService,
    sessions: Arc<SessionService>,
    storage: Arc<SqliteStorage>,
    auth: Arc<TokenAuthProvider>,
}

async fn setup() -> Fixture {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let audit = Arc::new(AuditSink::new(storage.clone()));
    let sessions = Arc::new(SessionService::new(storage.clone(), audit.clone()));
    let auth = Arc::new(TokenAuthProvider::new(storage.clone()));
    let invites = InviteService::new(storage.clone(), sessions.clone(), audit);
    Fixture {
        invites,
        sessions,
        storage,
        auth,
    }
}

async fn seed_user(fx: &Fixture, name: &str) -> User {
    let (_, token) = fx.storage.create_user(name, false).await.unwrap();
    fx.auth.validate(&token).await.unwrap()
}

async fn seed_session(fx: &Fixture, owner: &User) -> Session {
    fx.sessions
        .create_session(owner, "local-shell", InputMode::Multiplexed)
        .await
        .unwrap()
}

fn default_params(role: Role) -> CreateInviteParams {
    CreateInviteParams {
        role,
        max_uses: 3,
        expires_in_minutes: Some(60),
        expires_in_secs: None,
        expires_at: None,
    }
}

// ---------------------------------------------------------------------------
// InviteService::create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_happy_path_returns_raw_token_and_row() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    let result = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    assert!(!result.token.is_empty(), "raw token must flow through");
    assert_eq!(result.role, Role::Operator);
    assert_eq!(result.max_uses, 3);
    assert_eq!(result.session_id, session.id);
    assert!(result.expires_at.is_some());
}

#[tokio::test]
async fn create_rejects_owner_role_with_invalid_input() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    let err = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Owner))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[tokio::test]
async fn create_rejects_zero_and_overflow_max_uses() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    let mut params = default_params(Role::Operator);
    params.max_uses = 0;
    let err = fx
        .invites
        .create(&owner, &session.id, params.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));

    params.max_uses = MAX_INVITE_USES + 1;
    let err = fx
        .invites
        .create(&owner, &session.id, params)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[tokio::test]
async fn create_clamps_huge_ttl_to_ceiling() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    // Ask for 30 days; the service clamps to MAX_INVITE_TTL_MINUTES (7d).
    let result = fx
        .invites
        .create(
            &owner,
            &session.id,
            CreateInviteParams {
                role: Role::Viewer,
                max_uses: 1,
                expires_in_minutes: Some(30 * 24 * 60),
                expires_in_secs: None,
                expires_at: None,
            },
        )
        .await
        .unwrap();

    let expires_at = result.expires_at.expect("TTL must materialize expiry");
    let ceiling = Utc::now() + Duration::minutes(MAX_INVITE_TTL_MINUTES);
    // Allow a one-minute fudge for clock drift between the service
    // call and the assertion.
    assert!(
        expires_at <= ceiling + Duration::minutes(1),
        "expiry {expires_at} exceeded hard ceiling {ceiling}"
    );
}

#[tokio::test]
async fn create_rejects_negative_ttl() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    let err = fx
        .invites
        .create(
            &owner,
            &session.id,
            CreateInviteParams {
                role: Role::Viewer,
                max_uses: 1,
                expires_in_minutes: Some(-10),
                expires_in_secs: None,
                expires_at: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[tokio::test]
async fn create_rejects_past_absolute_expiry() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    let err = fx
        .invites
        .create(
            &owner,
            &session.id,
            CreateInviteParams {
                role: Role::Viewer,
                max_uses: 1,
                expires_in_minutes: None,
                expires_in_secs: None,
                expires_at: Some(Utc::now() - Duration::minutes(5)),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[tokio::test]
async fn create_rejects_absolute_expiry_beyond_ceiling() {
    // Regression for the TTL-bypass finding: `MAX_INVITE_TTL_MINUTES`
    // was only enforced on the relative `expires_in_minutes` path
    // (via `.min(...)` clamp). A direct-API caller could pass an
    // `expires_at` timestamp weeks or months in the future and the
    // service would accept it unchanged, effectively disabling the
    // hard ceiling that the UI slider respects. The fix rejects
    // out-of-range absolute timestamps with 400 so the ceiling is
    // single-point enforced across both input paths.
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    // 30 days out — comfortably beyond the 7-day ceiling.
    let too_far = Utc::now() + Duration::days(30);
    let err = fx
        .invites
        .create(
            &owner,
            &session.id,
            CreateInviteParams {
                role: Role::Viewer,
                max_uses: 1,
                expires_in_minutes: None,
                expires_in_secs: None,
                expires_at: Some(too_far),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::InvalidInput(_)),
        "30-day absolute expiry must reject, got {err:?}"
    );

    // Sanity: an `expires_at` exactly inside the ceiling still
    // works, so the reject is a strict upper bound, not a
    // blanket disable of the absolute-timestamp path.
    let inside = Utc::now() + Duration::minutes(MAX_INVITE_TTL_MINUTES - 5);
    let ok = fx
        .invites
        .create(
            &owner,
            &session.id,
            CreateInviteParams {
                role: Role::Viewer,
                max_uses: 1,
                expires_in_minutes: None,
                expires_in_secs: None,
                expires_at: Some(inside),
            },
        )
        .await
        .unwrap();
    assert!(ok.expires_at.is_some());
}

#[tokio::test]
async fn create_rejects_non_owner() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let stranger = seed_user(&fx, "stranger").await;
    let session = seed_session(&fx, &owner).await;

    let err = fx
        .invites
        .create(&stranger, &session.id, default_params(Role::Operator))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)));
}

#[tokio::test]
async fn create_against_closed_session_returns_gone() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    fx.sessions
        .close_session(&session.id, CloseReason::Owner, Some(&owner))
        .await
        .unwrap();

    let err = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SessionClosed(_)));
}

// ---------------------------------------------------------------------------
// InviteService::redeem
// ---------------------------------------------------------------------------

#[tokio::test]
async fn redeem_without_caller_mints_scoped_guest() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    let created = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    let result = fx.invites.redeem(None, &created.token).await.unwrap();
    let issued = result.issued_token.expect("guest mint must return token");

    // The minted user is scoped to the redeemed session.
    let guest = fx.auth.validate(&issued).await.unwrap();
    assert_eq!(
        guest.scoped_session_id.as_deref(),
        Some(session.id.as_str())
    );
    assert_eq!(result.role, Role::Operator);
}

#[tokio::test]
async fn redeem_by_owner_is_noop_and_preserves_uses() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    let created = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    let result = fx
        .invites
        .redeem(Some(owner.clone()), &created.token)
        .await
        .unwrap();
    assert!(result.issued_token.is_none(), "owner reuses identity");
    assert_eq!(
        result.role,
        Role::Owner,
        "owner keeps their role, not the invite's"
    );

    // Invite's used_count must stay at 0 — sanity-checking a link
    // should never burn a use.
    let preview = fx.storage.find_invite(&created.token).await.unwrap();
    assert_eq!(preview.used_count, 0);
}

#[tokio::test]
async fn redeem_by_scoped_guest_for_other_session_is_forbidden() {
    let fx = setup().await;
    let owner_a = seed_user(&fx, "owner-a").await;
    let owner_b = seed_user(&fx, "owner-b").await;
    let session_a = seed_session(&fx, &owner_a).await;
    let session_b = seed_session(&fx, &owner_b).await;

    // Mint a guest pinned to session A.
    let invite_a = fx
        .invites
        .create(&owner_a, &session_a.id, default_params(Role::Operator))
        .await
        .unwrap();
    let redeem_a = fx.invites.redeem(None, &invite_a.token).await.unwrap();
    let guest_token = redeem_a.issued_token.unwrap();
    let guest = fx.auth.validate(&guest_token).await.unwrap();

    // Now owner B mints an invite and the scoped guest tries to
    // redeem it — must be rejected with PermissionDenied.
    let invite_b = fx
        .invites
        .create(&owner_b, &session_b.id, default_params(Role::Operator))
        .await
        .unwrap();
    let err = fx
        .invites
        .redeem(Some(guest), &invite_b.token)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)));

    // Critically: the invite must NOT have been consumed.
    let preview = fx.storage.find_invite(&invite_b.token).await.unwrap();
    assert_eq!(preview.used_count, 0);
}

#[tokio::test]
async fn redeem_against_closed_session_returns_gone_without_burn() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    let created = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    fx.sessions
        .close_session(&session.id, CloseReason::Owner, Some(&owner))
        .await
        .unwrap();

    let err = fx.invites.redeem(None, &created.token).await.unwrap_err();
    assert!(matches!(err, Error::SessionClosed(_)));

    // Invite row must still be intact — we want to fail without
    // burning a use so the operator can re-use it against a new
    // session or retract it cleanly.
    let preview = fx.storage.find_invite(&created.token).await.unwrap();
    assert_eq!(preview.used_count, 0);
}

#[tokio::test]
async fn redeem_by_existing_member_of_closed_session_returns_gone() {
    // Regression for H5: the "owner/existing member clicks their own
    // invite" short path used to rebuild the active-session check
    // out of two independent queries (`get_session_required` +
    // `list_participants`). With those two queries, a concurrent
    // `close_session` committing between them could cause this
    // branch to return a happy `RedeemResult` against a session
    // that was *already* closed — the caller then navigated to a
    // dead session and ate a WS-layer bounce instead of the clean
    // `SessionClosed` error the API contract promises.
    //
    // The fix routes the short path through
    // `SessionService::find_active_participant_role`, a single
    // SELECT-JOIN that evaluates both predicates (`left_at IS NULL`
    // and `sessions.status = 'active'`) in one MVCC snapshot. Under
    // that shape, a pre-closed session is structurally unreachable
    // from the short path: the join returns `None` and we fall
    // through to the outer active-session rejection.
    //
    // This test deterministically simulates the race outcome — the
    // session is closed *before* redeem runs, so the short path
    // sees a closed session from the start. A raced close would
    // resolve to the same code path via the same lookup.
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    let created = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    // Close the session BEFORE the owner (who is already a
    // participant) tries to redeem. The pre-H5 code would still
    // have caught this via the up-front `get_session_required`
    // check; the new shape catches it via the atomic lookup even
    // if the up-front check were removed or raced.
    fx.sessions
        .close_session(&session.id, CloseReason::Owner, Some(&owner))
        .await
        .unwrap();

    // Owner is an existing participant of a now-closed session.
    // The redeem MUST NOT return a happy short-path success —
    // that would lie to the caller about the session being
    // joinable. Expect a SessionClosed error (→ 410 Gone).
    let err = fx
        .invites
        .redeem(Some(owner.clone()), &created.token)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::SessionClosed(_)),
        "existing member of a closed session must get SessionClosed, got {err:?}"
    );

    // And the invite's use count must stay pristine — the failure
    // path is still a no-op so a future retry against a new
    // session (or a revoke) has clean state to work with.
    let preview = fx.storage.find_invite(&created.token).await.unwrap();
    assert_eq!(preview.used_count, 0);
}

// ---------------------------------------------------------------------------
// InviteService::list_for_session / ::revoke
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_for_session_returns_all_invites_with_remaining_uses() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    // Mint three invites, consume one partially so remaining_uses is
    // interesting (2 remaining out of 3).
    let a = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();
    let _b = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Viewer))
        .await
        .unwrap();
    let _c = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Viewer))
        .await
        .unwrap();

    // Burn one use on invite A.
    fx.invites.redeem(None, &a.token).await.unwrap();

    let rows = fx
        .invites
        .list_for_session(&owner, &session.id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);

    // Every row carries a computed `remaining_uses`. Pick out the
    // row we burned a use on and verify the arithmetic.
    let consumed_row = rows
        .iter()
        .find(|r| r.used_count == 1)
        .expect("consumed row present");
    assert_eq!(consumed_row.max_uses, 3);
    assert_eq!(consumed_row.remaining_uses, 2);
    assert_eq!(consumed_row.token_prefix.len(), 8);
    assert!(consumed_row.created_at.is_some());
}

#[tokio::test]
async fn list_for_session_rejects_non_owner() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let stranger = seed_user(&fx, "stranger").await;
    let session = seed_session(&fx, &owner).await;

    let err = fx
        .invites
        .list_for_session(&stranger, &session.id)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)));
}

#[tokio::test]
async fn list_for_session_works_on_closed_session() {
    // Post-mortem use case: an admin wants to see who was invited
    // to a session that has since been closed. `list_for_session`
    // uses `require_owner`, not `require_active_owned`, so this path
    // must still return rows.
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    fx.invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    fx.sessions
        .close_session(&session.id, CloseReason::Owner, Some(&owner))
        .await
        .unwrap();

    let rows = fx
        .invites
        .list_for_session(&owner, &session.id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn revoke_hard_deletes_and_blocks_future_redeem() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;
    let created = fx
        .invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    let rows = fx
        .invites
        .list_for_session(&owner, &session.id)
        .await
        .unwrap();
    let sha = rows[0].token_sha256.clone();

    fx.invites.revoke(&owner, &session.id, &sha).await.unwrap();

    // Row is gone from listings.
    let rows = fx
        .invites
        .list_for_session(&owner, &session.id)
        .await
        .unwrap();
    assert!(rows.is_empty());

    // And the raw token no longer redeems — the whole point.
    let err = fx.invites.redeem(None, &created.token).await.unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[tokio::test]
async fn revoke_rejects_non_owner() {
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let stranger = seed_user(&fx, "stranger").await;
    let session = seed_session(&fx, &owner).await;
    fx.invites
        .create(&owner, &session.id, default_params(Role::Operator))
        .await
        .unwrap();

    let rows = fx
        .invites
        .list_for_session(&owner, &session.id)
        .await
        .unwrap();
    let sha = rows[0].token_sha256.clone();

    let err = fx
        .invites
        .revoke(&stranger, &session.id, &sha)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)));
}

#[tokio::test]
async fn revoke_cross_session_is_idempotent_noop() {
    // Probing for another session's invites must be indistinguishable
    // from "never existed": both read as Ok(()) on the wire so the
    // caller cannot enumerate cross-session invites. Critically, the
    // target invite in session A must remain untouched — idempotency
    // is about the response shape, not about granting cross-session
    // write access.
    let fx = setup().await;
    let owner_a = seed_user(&fx, "owner-a").await;
    let owner_b = seed_user(&fx, "owner-b").await;
    let session_a = seed_session(&fx, &owner_a).await;
    let session_b = seed_session(&fx, &owner_b).await;

    // Mint an invite in session A.
    fx.invites
        .create(&owner_a, &session_a.id, default_params(Role::Operator))
        .await
        .unwrap();
    let rows = fx
        .invites
        .list_for_session(&owner_a, &session_a.id)
        .await
        .unwrap();
    let sha_a = rows[0].token_sha256.clone();

    // Owner B tries to revoke it via their own session id — no-op.
    fx.invites
        .revoke(&owner_b, &session_b.id, &sha_a)
        .await
        .unwrap();

    // Owner A's invite must still be there — the probe returned OK,
    // but the actual row is untouched.
    let rows = fx
        .invites
        .list_for_session(&owner_a, &session_a.id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "cross-session probe must not delete");
}

#[tokio::test]
async fn revoke_unknown_sha_is_idempotent_noop() {
    // A double-revoke or a request against a fabricated sha is a
    // no-op so the HTTP layer can answer 204 without leaking whether
    // the invite ever existed.
    let fx = setup().await;
    let owner = seed_user(&fx, "owner").await;
    let session = seed_session(&fx, &owner).await;

    fx.invites
        .revoke(&owner, &session.id, &"0".repeat(64))
        .await
        .unwrap();
}
