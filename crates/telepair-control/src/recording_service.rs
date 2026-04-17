use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use telepair_core::auth::token_sha256;
use telepair_core::error::{Error, Result};
use telepair_core::recording::{AsciicastHeader, RecordingConfig, RecordingRow, RecordingShareRow};
use telepair_core::storage::{SqliteStorage, Storage};

/// Business logic layer for session recordings. Sits between the REST
/// API (Task 8) and the storage + hub layers, centralizing conflict
/// checks, TTL computation, file-path resolution, and share-token
/// lifecycle.
pub struct RecordingService {
    storage: Arc<SqliteStorage>,
    config: RecordingConfig,
}

impl RecordingService {
    pub fn new(storage: Arc<SqliteStorage>, config: RecordingConfig) -> Self {
        Self { storage, config }
    }

    pub fn config(&self) -> &RecordingConfig {
        &self.config
    }

    /// Decide whether a session should be recorded. The server-wide
    /// `config.enabled` flag is the master switch; a per-session
    /// override (`Some(true)` / `Some(false)`) can opt individual
    /// sessions in or out. `None` means "follow the global default".
    pub fn should_record(&self, session_override: Option<bool>) -> bool {
        match session_override {
            Some(v) => v && self.config.enabled,
            None => self.config.enabled,
        }
    }

    /// Full filesystem path for a recording file.
    /// Layout: `<dir>/<recording_id>.cast`
    pub fn recording_file_path(&self, recording_id: &str) -> PathBuf {
        self.config.dir.join(format!("{recording_id}.cast"))
    }

    /// Compute the `expires_at` timestamp based on the configured TTL.
    /// Returns `None` when TTL is 0 (permanent).
    pub fn compute_expires_at(&self) -> Option<String> {
        if self.config.ttl_days == 0 {
            return None;
        }
        let expires = Utc::now() + chrono::Duration::days(i64::from(self.config.ttl_days));
        Some(expires.to_rfc3339())
    }

    /// Build an asciicast v2 header with telepair metadata.
    pub fn build_header(
        &self,
        session_id: &str,
        recording_id: &str,
        width: u16,
        height: u16,
    ) -> AsciicastHeader {
        let mut env = std::collections::HashMap::new();
        env.insert("SHELL".to_string(), "/bin/bash".to_string());
        env.insert("TERM".to_string(), "xterm-256color".to_string());

        let telepair = serde_json::json!({
            "session_id": session_id,
            "recording_id": recording_id,
        });

        AsciicastHeader {
            version: 2,
            width,
            height,
            timestamp: Utc::now().timestamp(),
            env,
            telepair,
        }
    }

    /// Create a new recording for a session. Enforces the "at most one
    /// active recording per session" invariant by checking for an
    /// existing `status = 'recording'` row first. Returns
    /// `Error::Conflict` if one already exists.
    pub async fn create_recording(
        &self,
        session_id: &str,
        created_by: Uuid,
        width: i64,
        height: i64,
    ) -> Result<RecordingRow> {
        // Conflict check: one active recording per session.
        if let Some(existing) = self.storage.find_active_recording(session_id).await? {
            return Err(Error::Conflict(format!(
                "session {session_id} already has an active recording: {}",
                existing.id
            )));
        }

        let recording_id = nanoid::nanoid!(21);
        let file_path = format!("{recording_id}.cast");
        let expires_at = self.compute_expires_at();

        // Ensure the recordings directory exists.
        std::fs::create_dir_all(&self.config.dir).map_err(|e| {
            Error::Internal(format!(
                "failed to create recordings dir {}: {e}",
                self.config.dir.display()
            ))
        })?;

        self.storage
            .create_recording(
                &recording_id,
                session_id,
                created_by,
                width,
                height,
                &file_path,
                expires_at.as_deref(),
            )
            .await
    }

    /// Look up a recording by id.
    pub async fn get_recording(&self, id: &str) -> Result<Option<RecordingRow>> {
        self.storage.get_recording(id).await
    }

    /// List all recordings created by a specific user.
    pub async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<RecordingRow>> {
        self.storage.list_recordings_for_user(user_id).await
    }

    /// List every recording in the system (admin-only gate is the
    /// caller's responsibility).
    pub async fn list_all(&self) -> Result<Vec<RecordingRow>> {
        self.storage.list_all_recordings().await
    }

    /// Hard-delete a recording row and its associated file on disk.
    pub async fn delete_recording(&self, id: &str) -> Result<()> {
        // Best-effort file removal. A missing file is not an error — the
        // DB row is the authoritative "exists" record.
        let path = self.recording_file_path(id);
        let _ = std::fs::remove_file(&path);

        self.storage.delete_recording(id).await
    }

    /// Clear `expires_at` so the recording never expires.
    pub async fn set_permanent(&self, id: &str) -> Result<()> {
        self.storage.set_recording_permanent(id).await
    }

    /// Set or update `expires_at` to a specific RFC3339 timestamp.
    pub async fn set_expiry(&self, id: &str, expires_at: &str) -> Result<()> {
        self.storage.set_recording_expiry(id, expires_at).await
    }

    /// Create a share token for a recording. Returns `(raw_token,
    /// RecordingShareRow)` — the raw token is only visible at mint
    /// time; the DB stores its SHA-256 digest.
    pub async fn create_share(
        &self,
        recording_id: &str,
        max_uses: i64,
        expires_at: Option<&str>,
    ) -> Result<(String, RecordingShareRow)> {
        // Verify the recording exists.
        self.storage
            .get_recording(recording_id)
            .await?
            .ok_or_else(|| Error::InvalidInput(format!("recording not found: {recording_id}")))?;

        let raw_token = nanoid::nanoid!(32);
        let sha256_hex = token_sha256(&raw_token);

        let row = self
            .storage
            .create_recording_share(recording_id, &sha256_hex, max_uses, expires_at)
            .await?;

        Ok((raw_token, row))
    }

    /// Atomically validate and consume a share token for the given
    /// recording. Single SQL UPDATE that increments `used_count`
    /// only if the token exists, belongs to `expected_recording_id`,
    /// has not expired, and has remaining uses. Returns the
    /// post-increment share row on success.
    ///
    /// Threading the recording id through the storage layer (instead
    /// of validating it after the increment in the HTTP handler)
    /// closes the previous TOCTOU window where two concurrent calls
    /// could both pass an application-level `used_count < max_uses`
    /// check, and prevents a holder of one recording's share from
    /// burning the quota by hitting another recording's URL.
    pub async fn validate_share_token(
        &self,
        raw_token: &str,
        expected_recording_id: &str,
    ) -> Result<RecordingShareRow> {
        let sha256_hex = token_sha256(raw_token);
        self.storage
            .consume_recording_share(&sha256_hex, expected_recording_id)
            .await?
            .ok_or_else(|| Error::InvalidInput("invalid, expired, or exhausted share token".into()))
    }

    /// List all share tokens for a recording.
    pub async fn list_shares(&self, recording_id: &str) -> Result<Vec<RecordingShareRow>> {
        self.storage.list_recording_shares(recording_id).await
    }

    /// Hard-delete a share token by its SHA-256 digest. The admin UI
    /// already has the digest from `list_shares`; the raw token is
    /// only ever returned at mint time. Accepting the digest avoids
    /// putting the raw secret in DELETE URLs (where it would land in
    /// access logs and Referer headers) and prevents the previous
    /// double-hash bug where the handler treated the URL parameter as
    /// a raw token and re-hashed it before lookup.
    pub async fn delete_share_by_sha256(&self, token_sha256: &str) -> Result<()> {
        self.storage.delete_recording_share(token_sha256).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &std::path::Path) -> RecordingConfig {
        RecordingConfig {
            enabled: true,
            ttl_days: 30,
            dir: dir.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn should_record_follows_global_default_when_no_override() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage.clone(),
            RecordingConfig {
                enabled: true,
                ttl_days: 30,
                dir: PathBuf::from("/tmp"),
            },
        );
        assert!(svc.should_record(None));

        let svc2 = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: false,
                ttl_days: 30,
                dir: PathBuf::from("/tmp"),
            },
        );
        assert!(!svc2.should_record(None));
    }

    #[tokio::test]
    async fn should_record_override_true_requires_global_enabled() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: false,
                ttl_days: 30,
                dir: PathBuf::from("/tmp"),
            },
        );
        // Override true but global disabled = no recording.
        assert!(!svc.should_record(Some(true)));
    }

    #[tokio::test]
    async fn should_record_override_false_disables_even_when_global_enabled() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: true,
                ttl_days: 30,
                dir: PathBuf::from("/tmp"),
            },
        );
        assert!(!svc.should_record(Some(false)));
    }

    #[tokio::test]
    async fn recording_file_path_builds_correct_path() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: true,
                ttl_days: 30,
                dir: PathBuf::from("/data/recordings"),
            },
        );
        let path = svc.recording_file_path("abc123");
        assert_eq!(path, PathBuf::from("/data/recordings/abc123.cast"));
    }

    #[tokio::test]
    async fn compute_expires_at_returns_none_for_zero_ttl() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: true,
                ttl_days: 0,
                dir: PathBuf::from("/tmp"),
            },
        );
        assert!(svc.compute_expires_at().is_none());
    }

    #[tokio::test]
    async fn compute_expires_at_returns_some_for_positive_ttl() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: true,
                ttl_days: 7,
                dir: PathBuf::from("/tmp"),
            },
        );
        let result = svc.compute_expires_at();
        assert!(result.is_some());
        let parsed = chrono::DateTime::parse_from_rfc3339(&result.unwrap());
        assert!(parsed.is_ok());
    }

    #[tokio::test]
    async fn build_header_populates_correctly() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(
            storage,
            RecordingConfig {
                enabled: true,
                ttl_days: 30,
                dir: PathBuf::from("/tmp"),
            },
        );
        let header = svc.build_header("sess-1", "rec-1", 80, 24);
        assert_eq!(header.version, 2);
        assert_eq!(header.width, 80);
        assert_eq!(header.height, 24);
        assert_eq!(header.telepair["session_id"], "sess-1");
        assert_eq!(header.telepair["recording_id"], "rec-1");
    }

    #[test]
    fn token_sha256_is_deterministic() {
        let a = token_sha256("test-token");
        let b = token_sha256("test-token");
        assert_eq!(a, b);
        assert_ne!(token_sha256("other"), a);
    }

    #[tokio::test]
    async fn create_recording_enforces_single_active_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(storage.clone(), test_config(dir.path()));

        // Seed a user and session.
        let (user, _) = storage.create_user("tester", false).await.unwrap();
        let session = storage
            .create_session_with_owner(
                user.id,
                "local-shell",
                telepair_core::session::InputMode::Serialized,
                None,
            )
            .await
            .unwrap();

        // First recording succeeds.
        let rec = svc
            .create_recording(&session.id, user.id, 80, 24)
            .await
            .unwrap();
        assert_eq!(rec.session_id, session.id);

        // Second recording for the same session is a conflict.
        let err = svc
            .create_recording(&session.id, user.id, 80, 24)
            .await
            .expect_err("duplicate active recording must conflict");
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[tokio::test]
    async fn create_share_and_validate_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(storage.clone(), test_config(dir.path()));

        let (user, _) = storage.create_user("tester", false).await.unwrap();
        let session = storage
            .create_session_with_owner(
                user.id,
                "local-shell",
                telepair_core::session::InputMode::Serialized,
                None,
            )
            .await
            .unwrap();
        let rec = svc
            .create_recording(&session.id, user.id, 80, 24)
            .await
            .unwrap();

        // Create a share with max_uses = 2.
        let (raw, share) = svc.create_share(&rec.id, 2, None).await.unwrap();
        assert_eq!(share.max_uses, 2);
        assert_eq!(share.used_count, 0);

        // First validation succeeds and increments usage.
        let validated = svc.validate_share_token(&raw, &rec.id).await.unwrap();
        assert_eq!(validated.recording_id, rec.id);
        assert_eq!(validated.used_count, 1);

        // Second validation also succeeds.
        svc.validate_share_token(&raw, &rec.id).await.unwrap();

        // Third validation should fail (max_uses exhausted).
        let err = svc
            .validate_share_token(&raw, &rec.id)
            .await
            .expect_err("exhausted share must fail");
        assert!(matches!(err, Error::InvalidInput(_)));

        // Wrong recording id must fail without consuming a use.
        let err = svc
            .validate_share_token(&raw, "some-other-recording")
            .await
            .expect_err("mismatched recording id must fail");
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[tokio::test]
    async fn delete_share_removes_token() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(storage.clone(), test_config(dir.path()));

        let (user, _) = storage.create_user("tester", false).await.unwrap();
        let session = storage
            .create_session_with_owner(
                user.id,
                "local-shell",
                telepair_core::session::InputMode::Serialized,
                None,
            )
            .await
            .unwrap();
        let rec = svc
            .create_recording(&session.id, user.id, 80, 24)
            .await
            .unwrap();

        let (raw, share) = svc.create_share(&rec.id, 0, None).await.unwrap();
        svc.delete_share_by_sha256(&share.token_sha256)
            .await
            .unwrap();

        // Token should no longer validate.
        let err = svc
            .validate_share_token(&raw, &rec.id)
            .await
            .expect_err("deleted share must fail");
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[tokio::test]
    async fn list_shares_returns_all_for_recording() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(storage.clone(), test_config(dir.path()));

        let (user, _) = storage.create_user("tester", false).await.unwrap();
        let session = storage
            .create_session_with_owner(
                user.id,
                "local-shell",
                telepair_core::session::InputMode::Serialized,
                None,
            )
            .await
            .unwrap();
        let rec = svc
            .create_recording(&session.id, user.id, 80, 24)
            .await
            .unwrap();

        svc.create_share(&rec.id, 5, None).await.unwrap();
        svc.create_share(&rec.id, 10, None).await.unwrap();

        let shares = svc.list_shares(&rec.id).await.unwrap();
        assert_eq!(shares.len(), 2);
    }

    #[tokio::test]
    async fn create_share_for_nonexistent_recording_errors() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let svc = RecordingService::new(storage, test_config(dir.path()));

        let err = svc
            .create_share("nonexistent", 5, None)
            .await
            .expect_err("nonexistent recording must error");
        assert!(matches!(err, Error::InvalidInput(_)));
    }
}
