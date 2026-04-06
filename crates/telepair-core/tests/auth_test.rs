use std::sync::Arc;

use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> (TokenAuthProvider, String) {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, token) = store.create_user("test-user", false).await.unwrap();
    let auth = TokenAuthProvider::new(store);
    (auth, token)
}

#[tokio::test]
async fn valid_token_returns_user() {
    let (auth, token) = setup().await;
    let user = auth.validate(&token).await.unwrap();
    assert_eq!(user.name, "test-user");
}

#[tokio::test]
async fn invalid_token_returns_error() {
    let (auth, _) = setup().await;
    let result = auth.validate("bad-token").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn setup_initial_admin() {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let auth = TokenAuthProvider::new(store);
    let (user, token) = auth.setup_initial_admin("admin").await.unwrap();
    assert!(user.is_admin);

    let validated = auth.validate(&token).await.unwrap();
    assert_eq!(validated.id, user.id);
}

#[tokio::test]
async fn create_guest_returns_validatable_token() {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let auth = TokenAuthProvider::new(store);
    let (user, token) = auth.create_guest().await.unwrap();

    assert!(!user.is_admin, "guests must never have admin rights");
    assert!(
        user.name.starts_with("guest-"),
        "guest name should use guest- prefix, got {}",
        user.name
    );

    // The freshly issued token should authenticate as the same user —
    // this is the contract the redeem handler relies on to make
    // invite links "just work" without any prior registration.
    let validated = auth.validate(&token).await.unwrap();
    assert_eq!(validated.id, user.id);
}

#[tokio::test]
async fn create_guest_issues_unique_names() {
    // The invite flow calls create_guest once per redemption; two
    // consecutive calls must not collide on the UNIQUE users.name
    // constraint. This also indirectly exercises the retry loop:
    // a broken generator that always returned the same name would
    // blow up here.
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let auth = TokenAuthProvider::new(store);

    let (a, _) = auth.create_guest().await.unwrap();
    let (b, _) = auth.create_guest().await.unwrap();
    assert_ne!(a.id, b.id);
    assert_ne!(a.name, b.name);
}
