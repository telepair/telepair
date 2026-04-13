use std::collections::HashMap;
use std::sync::Arc;

use telepair_core::error::{Error, Result};
use telepair_core::session::{CreateUserTargetParams, UserTarget};
use telepair_core::storage::{SqliteStorage, Storage};
use uuid::Uuid;

pub struct CreateTargetParams {
    pub name: String,
    pub display: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub tags: Vec<String>,
}

pub struct UpdateTargetParams {
    pub display: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub tags: Vec<String>,
}

pub struct UserTargetService {
    storage: Arc<SqliteStorage>,
}

impl UserTargetService {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub async fn create(&self, user_id: Uuid, params: CreateTargetParams) -> Result<UserTarget> {
        // Validate: name, display, command must be non-blank
        if params.name.trim().is_empty() {
            return Err(Error::InvalidInput("name is required".into()));
        }
        if params.display.trim().is_empty() {
            return Err(Error::InvalidInput("display is required".into()));
        }
        if params.command.trim().is_empty() {
            return Err(Error::InvalidInput("command is required".into()));
        }
        self.storage
            .create_user_target(CreateUserTargetParams {
                user_id,
                name: params.name,
                display: params.display,
                command: params.command,
                args: params.args,
                env: params.env,
                tags: params.tags,
            })
            .await
    }

    pub async fn update(
        &self,
        id: &str,
        user_id: Uuid,
        params: UpdateTargetParams,
    ) -> Result<UserTarget> {
        if params.display.trim().is_empty() {
            return Err(Error::InvalidInput("display is required".into()));
        }
        if params.command.trim().is_empty() {
            return Err(Error::InvalidInput("command is required".into()));
        }
        self.storage
            .update_user_target(
                id,
                user_id,
                &params.display,
                &params.command,
                &params.args,
                &params.env,
                &params.tags,
            )
            .await
    }

    pub async fn delete(&self, id: &str, user_id: Uuid) -> Result<()> {
        self.storage.delete_user_target(id, user_id).await
    }

    pub async fn get(&self, id: &str, user_id: Uuid) -> Result<Option<UserTarget>> {
        let target = self.storage.find_user_target_by_id(id).await?;
        Ok(target.filter(|t| t.user_id == user_id))
    }

    pub async fn list(&self, user_id: Uuid) -> Result<Vec<UserTarget>> {
        self.storage.list_user_targets(user_id).await
    }

    /// Resolve a user target by its nanoid to its raw
    /// `(command, args, env)` tuple, **without** `${VAR}` expansion.
    ///
    /// Unlike `TargetEngine::resolve`, which expands `${VAR}` from the
    /// gateway process environment for admin-managed `targets.yaml`
    /// entries, user-owned targets must never touch the process env.
    /// Any authenticated non-guest user can persist arbitrary
    /// `command`/`args`/`env` strings through `/api/user-targets`, so
    /// honouring `${SMTP_PASS}` or `${DATABASE_URL}` here would let a
    /// normal user read the gateway's secrets by staging them into a
    /// PTY command line. The WS PTY spawn path passes these strings
    /// straight to the child process; literal `$VAR` tokens survive
    /// verbatim and hit the child shell's own env expansion instead,
    /// which is scoped to the target's own declared env.
    pub async fn resolve_by_id(
        &self,
        id: &str,
    ) -> Result<Option<(String, Vec<String>, HashMap<String, String>)>> {
        let Some(target) = self.storage.find_user_target_by_id(id).await? else {
            return Ok(None);
        };
        Ok(Some((target.command, target.args, target.env)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn user_target_service_crud() {
        let storage = Arc::new(
            telepair_core::storage::SqliteStorage::new_memory()
                .await
                .unwrap(),
        );
        let svc = UserTargetService::new(storage.clone());
        let (alice, _) = storage.create_user("alice", false).await.unwrap();
        let (bob, _) = storage.create_user("bob", false).await.unwrap();

        let p = CreateTargetParams {
            name: "vps".into(),
            display: "My VPS".into(),
            command: "ssh".into(),
            args: vec!["user@host".into()],
            env: Default::default(),
            tags: vec![],
        };
        let t = svc.create(alice.id, p).await.unwrap();
        assert_eq!(t.command, "ssh");
        assert_eq!(t.user_id, alice.id);

        // list
        let list = svc.list(alice.id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(svc.list(bob.id).await.unwrap().is_empty());

        // bob can't delete alice's target
        let err = svc.delete(&t.id, bob.id).await.unwrap_err();
        assert!(matches!(err, Error::PermissionDenied(_)));

        // alice can delete
        svc.delete(&t.id, alice.id).await.unwrap();
        assert!(svc.list(alice.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn resolve_by_id_does_not_expand_process_env_vars() {
        // Security: user-owned targets MUST NOT read the gateway
        // process environment. Any authenticated non-guest user can
        // persist arbitrary `command`/`args`/`env` strings through
        // `POST /api/user-targets`, so expanding `${SMTP_PASS}` here
        // would let them exfiltrate server secrets through a PTY
        // command line or child-process env.
        //
        // Rather than `set_var` (which is `unsafe` in edition 2024
        // and forbidden by `#![deny(unsafe_code)]` on this crate) we
        // rely on `CARGO`, which Cargo guarantees to set to its own
        // binary path when running `cargo test`. Its value always
        // contains `/` — if the resolver were still reading process
        // env, the literal `${CARGO}` in our target would become an
        // absolute path. We assert it does not.
        let cargo_path = std::env::var("CARGO").expect("CARGO is always set by `cargo test`");
        assert!(
            cargo_path.contains('/'),
            "sanity: CARGO should be an absolute path ({cargo_path})"
        );

        let storage = Arc::new(
            telepair_core::storage::SqliteStorage::new_memory()
                .await
                .unwrap(),
        );
        let svc = UserTargetService::new(storage.clone());
        let (user, _) = storage.create_user("dave", false).await.unwrap();

        let mut env = HashMap::new();
        env.insert("EXTRA".into(), "${CARGO}".into());

        let p = CreateTargetParams {
            name: "srv".into(),
            display: "Srv".into(),
            command: "${CARGO}".into(),
            args: vec!["-p".into(), "${CARGO}".into(), "host".into()],
            env,
            tags: vec![],
        };
        let t = svc.create(user.id, p).await.unwrap();
        let (cmd, args, resolved_env) = svc.resolve_by_id(&t.id).await.unwrap().unwrap();

        // Every field must survive as the literal `${CARGO}` —
        // process env must NOT leak into any of them.
        assert_eq!(cmd, "${CARGO}", "command must not expand process env");
        assert_eq!(args[1], "${CARGO}", "args must not expand process env");
        assert_eq!(
            resolved_env.get("EXTRA").map(String::as_str),
            Some("${CARGO}"),
            "env values must not expand process env"
        );
        // Belt-and-braces: the expanded value always contains '/',
        // the literal token never does.
        assert!(!cmd.contains('/'));
        assert!(!args[1].contains('/'));
        assert!(!resolved_env["EXTRA"].contains('/'));
    }

    // ── Referential guard against active sessions ─────────────────

    async fn seed_target(svc: &UserTargetService, user_id: uuid::Uuid) -> String {
        let p = CreateTargetParams {
            name: "vps".into(),
            display: "VPS".into(),
            command: "ssh".into(),
            args: vec!["user@host".into()],
            env: Default::default(),
            tags: vec![],
        };
        svc.create(user_id, p).await.unwrap().id
    }

    #[tokio::test]
    async fn update_rejects_when_active_session_references_target() {
        // Regression: `UserTargetService::update` used to succeed
        // unconditionally as long as the caller owned the row, so an
        // owner editing a target mid-session would silently shift the
        // command/args/env out from under the running PTY. The new
        // CAS in `update_user_target` gates on `NOT EXISTS (active
        // session referencing this id)`, so this call must fail with
        // `Error::Conflict` and a human-actionable message.
        let storage = Arc::new(
            telepair_core::storage::SqliteStorage::new_memory()
                .await
                .unwrap(),
        );
        let svc = UserTargetService::new(storage.clone());
        let (user, _) = storage.create_user("alice", false).await.unwrap();
        let target_id = seed_target(&svc, user.id).await;

        // Register a live session against this target_id.
        storage
            .create_session_with_owner(
                user.id,
                "vps",
                telepair_core::session::InputMode::Serialized,
                Some(&target_id),
            )
            .await
            .unwrap();

        let err = svc
            .update(
                &target_id,
                user.id,
                UpdateTargetParams {
                    display: "New Display".into(),
                    command: "ssh".into(),
                    args: vec!["newhost".into()],
                    env: Default::default(),
                    tags: vec![],
                },
            )
            .await
            .expect_err("update must be blocked while a session is active");

        match err {
            Error::Conflict(msg) => {
                assert!(
                    msg.contains("active session"),
                    "error message should mention active session: {msg}"
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        // Sanity: the target row is unchanged.
        let row = storage
            .find_user_target_by_id(&target_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.display, "VPS");
        assert_eq!(row.args, vec!["user@host".to_string()]);
    }

    #[tokio::test]
    async fn delete_rejects_when_active_session_references_target() {
        // Companion regression: `delete` must be blocked while a live
        // session still points at the target, otherwise the next WS
        // attach would orphan the session (`user_targets` row gone,
        // WS handler `cleanup_orphan_session` on resolve miss).
        let storage = Arc::new(
            telepair_core::storage::SqliteStorage::new_memory()
                .await
                .unwrap(),
        );
        let svc = UserTargetService::new(storage.clone());
        let (user, _) = storage.create_user("alice", false).await.unwrap();
        let target_id = seed_target(&svc, user.id).await;

        storage
            .create_session_with_owner(
                user.id,
                "vps",
                telepair_core::session::InputMode::Serialized,
                Some(&target_id),
            )
            .await
            .unwrap();

        let err = svc
            .delete(&target_id, user.id)
            .await
            .expect_err("delete must be blocked while a session is active");
        assert!(
            matches!(err, Error::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
        // Row is still there.
        assert!(
            storage
                .find_user_target_by_id(&target_id)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn update_and_delete_succeed_after_session_closes() {
        // The guard must release once the referencing session is
        // closed — otherwise closed sessions would permanently pin
        // their target row, which would make the target unusable for
        // future sessions and give users no way out.
        let storage = Arc::new(
            telepair_core::storage::SqliteStorage::new_memory()
                .await
                .unwrap(),
        );
        let svc = UserTargetService::new(storage.clone());
        let (user, _) = storage.create_user("alice", false).await.unwrap();
        let target_id = seed_target(&svc, user.id).await;

        let session = storage
            .create_session_with_owner(
                user.id,
                "vps",
                telepair_core::session::InputMode::Serialized,
                Some(&target_id),
            )
            .await
            .unwrap();

        // Close the session.
        storage
            .close_session(&session.id, telepair_core::session::CloseReason::Owner)
            .await
            .unwrap();

        // Now update should succeed.
        svc.update(
            &target_id,
            user.id,
            UpdateTargetParams {
                display: "After Close".into(),
                command: "ssh".into(),
                args: vec!["newhost".into()],
                env: Default::default(),
                tags: vec![],
            },
        )
        .await
        .expect("update must succeed once the referencing session is closed");

        // And delete should too.
        svc.delete(&target_id, user.id)
            .await
            .expect("delete must succeed once the referencing session is closed");
        assert!(
            storage
                .find_user_target_by_id(&target_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn non_owner_still_gets_permission_denied() {
        // The classifier must preserve the old 403 semantics when the
        // real reason is "row exists but caller doesn't own it" —
        // otherwise a 409 "in use" message would leak row existence
        // to an unrelated user.
        let storage = Arc::new(
            telepair_core::storage::SqliteStorage::new_memory()
                .await
                .unwrap(),
        );
        let svc = UserTargetService::new(storage.clone());
        let (alice, _) = storage.create_user("alice", false).await.unwrap();
        let (bob, _) = storage.create_user("bob", false).await.unwrap();
        let target_id = seed_target(&svc, alice.id).await;

        // Bob tries to update/delete Alice's target.
        let err = svc
            .update(
                &target_id,
                bob.id,
                UpdateTargetParams {
                    display: "hijack".into(),
                    command: "sh".into(),
                    args: vec![],
                    env: Default::default(),
                    tags: vec![],
                },
            )
            .await
            .expect_err("non-owner update must fail");
        assert!(
            matches!(err, Error::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );

        let err = svc
            .delete(&target_id, bob.id)
            .await
            .expect_err("non-owner delete must fail");
        assert!(
            matches!(err, Error::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }
}
