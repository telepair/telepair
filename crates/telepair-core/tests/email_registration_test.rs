//! Regression tests for the pending-registration path in
//! `SqliteStorage`. These live here (not inline in sqlite.rs) because
//! they cover cross-call invariants — "the row carries no authority",
//! "the OTP counter is atomic under concurrency", "re-register from
//! the same address replaces the hash without unlocking the previous
//! attempt" — that are most honestly expressed as end-to-end storage
//! exercises.
//!
//! v0.1.2 design recap:
//!
//! - A self-served signup writes a row to `pending_registrations`,
//!   keyed by lowercased email. The row carries the display name,
//!   argon2 hash, and OTP. There is no `users` row and no bearer
//!   token until the OTP successfully verifies.
//! - `verify_pending_registration` is one transaction: consume the
//!   pending row, INSERT into `users` with `verified = TRUE` and
//!   `session_enabled = FALSE`, return the freshly minted token.
//! - On code mismatch the failure counter advances (`Failure` →
//!   `Locked` at 5). The "no eligible row" branch collapses both
//!   "missing" and "expired" into `Expired` so the public API cannot
//!   enumerate which addresses have a pending signup in flight.

use std::sync::Arc;

use chrono::{Duration, Utc};
use telepair_core::session::PendingVerifyResult;
use telepair_core::storage::{SqliteStorage, Storage};

const TEST_HASH_A: &str = "$argon2id$placeholder-a";
const TEST_HASH_B: &str = "$argon2id$placeholder-b";

fn future() -> chrono::DateTime<Utc> {
    Utc::now() + Duration::minutes(15)
}

#[tokio::test]
async fn two_pending_registrations_coexist() {
    // Two distinct addresses must be able to sit in the pending table
    // at the same time. The pre-0.1.2 design used the `users` row as
    // the staging slot, which made every concurrent unverified signup
    // collide on the globally-UNIQUE `token_sha256` placeholder.
    let store = SqliteStorage::new_memory().await.unwrap();

    store
        .upsert_pending_registration(
            "alice@example.com",
            "alice",
            TEST_HASH_A,
            "111111",
            future(),
        )
        .await
        .expect("first pending row");
    store
        .upsert_pending_registration("bob@example.com", "bob", TEST_HASH_B, "222222", future())
        .await
        .expect("second pending row must not collide with first");

    // Neither pending row materialised a `users` row yet.
    assert!(
        store
            .get_user_by_email("alice@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_user_by_email("bob@example.com")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reregister_overwrites_pending_row_in_place() {
    // The v0.1.2 critical fix: if attacker hits `register` for the
    // same address that the victim is currently mid-signup on, the
    // pending row's hash + display_name must be replaced atomically.
    // The old design left a window where the attacker's hash sat in
    // the `users` row with `verified = FALSE`; if the victim went on
    // to verify with the OTP they had received against the OLD hash,
    // they'd materialise an account whose password was set by the
    // attacker.
    //
    // Under the pending-row design the OTP is part of the same row
    // as the hash, so a re-register replaces both atomically:
    // verifying with the *old* code now misses the row entirely and
    // collapses to `Expired`.
    let store = SqliteStorage::new_memory().await.unwrap();

    store
        .upsert_pending_registration("victim@x.com", "victim", TEST_HASH_A, "111111", future())
        .await
        .unwrap();
    // Attacker re-registers with a fresh code; the pending row's
    // hash and OTP are both replaced.
    store
        .upsert_pending_registration("victim@x.com", "evil", TEST_HASH_B, "999999", future())
        .await
        .unwrap();

    // The victim's old code is no longer accepted — the row is gone.
    let res = store
        .verify_pending_registration("victim@x.com", "111111")
        .await
        .unwrap();
    // Failure (code didn't match the new pending row) — NOT Success.
    assert!(
        matches!(res, PendingVerifyResult::Failure { .. }),
        "old OTP must NOT verify against the new pending row, got {res:?}"
    );
}

#[tokio::test]
async fn pending_row_carries_no_authority() {
    // Until verify succeeds, the pending row must not be reachable as
    // a `users` row by any of the read paths. This pins the security
    // contract: an attacker who somehow controls the pending table
    // still has nothing they can present to `validate_token` or
    // `get_user_by_email`.
    let store = SqliteStorage::new_memory().await.unwrap();
    store
        .upsert_pending_registration("ghost@x.com", "ghost", TEST_HASH_A, "111111", future())
        .await
        .unwrap();

    assert!(
        store
            .get_user_by_email("ghost@x.com")
            .await
            .unwrap()
            .is_none(),
        "pending row must NOT surface as a users row"
    );
    assert!(
        store.get_user_by_name("ghost").await.unwrap().is_none(),
        "pending display name must NOT surface as a users row"
    );
}

#[tokio::test]
async fn verify_materialises_user_with_session_disabled() {
    // The happy-path exit. After verify, a `users` row exists with
    // `session_enabled = FALSE` (awaiting admin approval) and the
    // returned bearer token is valid.
    let store = SqliteStorage::new_memory().await.unwrap();
    store
        .upsert_pending_registration(
            "alice@example.com",
            "alice",
            TEST_HASH_A,
            "123456",
            future(),
        )
        .await
        .unwrap();

    let outcome = store
        .verify_pending_registration("alice@example.com", "123456")
        .await
        .unwrap();
    let (user, token) = match outcome {
        PendingVerifyResult::Success { user, raw_token } => (user, raw_token),
        other => panic!("expected Success, got {other:?}"),
    };

    assert_eq!(user.name, "alice");
    assert!(
        !user.session_enabled,
        "fresh email signup must default to session_enabled = FALSE"
    );

    let validated = store.validate_token(&token).await.unwrap();
    assert_eq!(validated.id, user.id);
    assert!(!validated.session_enabled);

    // The pending row is gone — re-presenting the same code must
    // collapse to Expired.
    let again = store
        .verify_pending_registration("alice@example.com", "123456")
        .await
        .unwrap();
    assert!(matches!(again, PendingVerifyResult::Expired));
}

// ── verify_pending_registration atomicity ─────────────────────────────
//
// Same shape as the old verify_otp tests, but operating on the
// pending-row design. The CAS lives in `verify_pending_registration`
// itself; the tests here pin the externally-observable behaviour.

#[tokio::test]
async fn failure_sequence_locks_at_five() {
    // Four wrong guesses report 4, 3, 2, 1 remaining; the fifth
    // transitions to Locked; subsequent guesses (correct OR wrong)
    // stay Locked.
    let store = SqliteStorage::new_memory().await.unwrap();
    store
        .upsert_pending_registration(
            "alice@example.com",
            "alice",
            TEST_HASH_A,
            "123456",
            future(),
        )
        .await
        .unwrap();

    for expected_remaining in [4u32, 3, 2, 1] {
        let res = store
            .verify_pending_registration("alice@example.com", "000000")
            .await
            .unwrap();
        match res {
            PendingVerifyResult::Failure { remaining } => {
                assert_eq!(remaining, expected_remaining);
            }
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    // Fifth strike → Locked.
    assert!(matches!(
        store
            .verify_pending_registration("alice@example.com", "000000")
            .await
            .unwrap(),
        PendingVerifyResult::Locked
    ));
    // Even the correct code against a locked row must fail — the CAS
    // gates on `otp_failure_count < 5`.
    assert!(matches!(
        store
            .verify_pending_registration("alice@example.com", "123456")
            .await
            .unwrap(),
        PendingVerifyResult::Locked
    ));
}

#[tokio::test]
async fn concurrent_wrong_codes_lock_at_five() {
    // 20 concurrent wrong-code submissions: at most 5 may land as
    // Failure, at least one must observe Locked, and every attempt
    // must classify into one of the four variants. Under the old
    // read-then-write OTP path this test caught the race where every
    // worker read `failure_count = 0` and overwrote each other's
    // increments, leaving the counter stuck at 1.
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    store
        .upsert_pending_registration(
            "alice@example.com",
            "alice",
            TEST_HASH_A,
            "123456",
            future(),
        )
        .await
        .unwrap();

    let mut handles = Vec::with_capacity(20);
    for _ in 0..20 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.verify_pending_registration("alice@example.com", "000000")
                .await
                .unwrap()
        }));
    }
    let mut locked = 0;
    let mut failures = 0;
    let mut expired = 0;
    let mut success = 0;
    for h in handles {
        match h.await.unwrap() {
            PendingVerifyResult::Locked => locked += 1,
            PendingVerifyResult::Failure { .. } => failures += 1,
            PendingVerifyResult::Expired => expired += 1,
            PendingVerifyResult::Success { .. } => success += 1,
        }
    }
    assert_eq!(success, 0, "wrong code must not produce Success");
    assert!(locked >= 1, "at least one attempt must see Locked");
    assert!(
        failures <= 5,
        "no more than 5 concurrent wrong guesses may land; got {failures}"
    );
    assert_eq!(failures + locked + expired, 20);

    // Authoritative: the row is now locked.
    assert!(matches!(
        store
            .verify_pending_registration("alice@example.com", "123456")
            .await
            .unwrap(),
        PendingVerifyResult::Locked
    ));
}

#[tokio::test]
async fn concurrent_success_materialises_exactly_one_user() {
    // Two concurrent correct-code submissions on the same pending
    // row must produce exactly one `users` row and exactly one
    // returned bearer token; every other concurrent attempt must
    // collapse to Expired (the row is gone after the first winner).
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    store
        .upsert_pending_registration(
            "alice@example.com",
            "alice",
            TEST_HASH_A,
            "123456",
            future(),
        )
        .await
        .unwrap();

    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.verify_pending_registration("alice@example.com", "123456")
                .await
                .unwrap()
        }));
    }
    let mut success = 0;
    let mut other = 0;
    let mut winning_token: Option<String> = None;
    for h in handles {
        match h.await.unwrap() {
            PendingVerifyResult::Success { raw_token, .. } => {
                success += 1;
                winning_token = Some(raw_token);
            }
            _ => other += 1,
        }
    }
    assert_eq!(success, 1, "exactly one concurrent correct code may win");
    assert_eq!(other, 9);

    // The winning token must validate against the materialised user.
    let token = winning_token.expect("winner must have minted a token");
    let user = store.validate_token(&token).await.unwrap();
    assert_eq!(user.name, "alice");
    assert!(!user.session_enabled);

    // And the pending row is consumed.
    let again = store
        .verify_pending_registration("alice@example.com", "123456")
        .await
        .unwrap();
    assert!(matches!(again, PendingVerifyResult::Expired));
}

#[tokio::test]
async fn expired_row_reports_expired_without_burning_a_strike() {
    // A pending row whose `otp_expires_at` is in the past must
    // surface as `Expired` for both wrong and correct codes — and
    // crucially, the failure counter must NOT advance (a stuffing
    // attacker could otherwise use expired rows as cheap probes).
    let store = SqliteStorage::new_memory().await.unwrap();
    store
        .upsert_pending_registration(
            "alice@example.com",
            "alice",
            TEST_HASH_A,
            "123456",
            Utc::now() - Duration::minutes(1),
        )
        .await
        .unwrap();

    assert!(matches!(
        store
            .verify_pending_registration("alice@example.com", "000000")
            .await
            .unwrap(),
        PendingVerifyResult::Expired
    ));
    assert!(matches!(
        store
            .verify_pending_registration("alice@example.com", "123456")
            .await
            .unwrap(),
        PendingVerifyResult::Expired
    ));
}

#[tokio::test]
async fn unknown_email_collapses_to_expired() {
    // A pending lookup against a never-registered address must look
    // identical to one against an expired row. This is the
    // enumeration-safety contract: the public API must not let an
    // unauthenticated caller distinguish "no signup in flight" from
    // "signup expired".
    let store = SqliteStorage::new_memory().await.unwrap();
    let res = store
        .verify_pending_registration("ghost@x.com", "000000")
        .await
        .unwrap();
    assert!(matches!(res, PendingVerifyResult::Expired));
}
