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
