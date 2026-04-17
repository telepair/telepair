use std::path::PathBuf;
use std::sync::Arc;

use telepair_core::storage::{SqliteStorage, Storage};

/// How many expired recordings to process per run. A batch cap keeps
/// each run O(batch) in memory rather than loading every expired row at
/// once when the server has been offline for a long period.
const BATCH_LIMIT: i64 = 100;

/// Spawn the background TTL cleaner that periodically deletes recordings
/// whose `expires_at` has passed. The task runs immediately on startup
/// (to clear any expired rows from a prior period), then once per hour.
///
/// Errors are logged but never propagate — a failure in one batch must
/// not crash the server or block the next scheduled run.
///
/// `dir` is the recording directory on disk; files matching
/// `{dir}/{recording_id}.cast` are removed before the DB row is deleted.
pub fn spawn_recording_cleaner(storage: Arc<SqliteStorage>, dir: PathBuf) {
    tokio::spawn(async move {
        // Run immediately on startup to handle any backlog from a
        // previous server run, then switch to the hourly cadence.
        run_once(&storage, &dir).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        // First tick fires immediately; skip it so we don't double-clean
        // on startup.
        interval.tick().await;
        loop {
            interval.tick().await;
            run_once(&storage, &dir).await;
        }
    });
}

async fn run_once(storage: &Arc<SqliteStorage>, dir: &std::path::Path) {
    let expired = match storage.list_expired_recordings(BATCH_LIMIT).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "TTL cleaner: failed to query expired recordings");
            return;
        }
    };

    if expired.is_empty() {
        return;
    }

    tracing::info!(
        count = expired.len(),
        "TTL cleaner: deleting expired recordings"
    );

    for rec in expired {
        // Best-effort file removal. A missing file is not an error — it
        // may have been cleaned up by another process or a previous
        // partial run. Any other I/O failure is logged as a warning and
        // we still proceed with the DB deletion so the row doesn't
        // accumulate indefinitely.
        if let Some(ref path_str) = rec.file_path {
            let path = PathBuf::from(path_str);
            // If the stored path is relative, resolve it against `dir`.
            let abs_path = if path.is_absolute() {
                path
            } else {
                dir.join(path)
            };
            match std::fs::remove_file(&abs_path) {
                Ok(()) => {
                    tracing::debug!(
                        recording_id = %rec.id,
                        path = %abs_path.display(),
                        "TTL cleaner: removed recording file"
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::warn!(
                        recording_id = %rec.id,
                        path = %abs_path.display(),
                        "TTL cleaner: recording file not found (already removed?)"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        recording_id = %rec.id,
                        path = %abs_path.display(),
                        error = %e,
                        "TTL cleaner: failed to remove recording file"
                    );
                }
            }
        }

        // Delete the DB row. Cascade deletes associated `recording_shares`
        // rows. A failure here is an error (the file is gone but the row
        // lingers) — log it so an operator can investigate and re-run.
        if let Err(e) = storage.delete_recording(&rec.id).await {
            tracing::error!(
                recording_id = %rec.id,
                error = %e,
                "TTL cleaner: failed to delete recording row"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telepair_core::session::InputMode;
    use telepair_core::storage::Storage;

    /// End-to-end check that the cleaner removes both the on-disk
    /// `.cast` file and the DB row for an expired recording, while
    /// leaving non-expired ones alone. This pins the contract that
    /// the recording id used in `file_path` matches the row id —
    /// the previous bug minted them separately and the cleaner
    /// wiped the row but left the file behind.
    #[tokio::test]
    async fn cleaner_removes_expired_file_and_row_but_spares_others() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let dir = tempfile::tempdir().unwrap();

        // Seed a user + session.
        let (user, _) = storage.create_user("ttluser", false).await.unwrap();
        let session = storage
            .create_session_with_owner(user.id, "default", InputMode::Serialized, None)
            .await
            .unwrap();

        // One expired recording: row + file that the cleaner should erase.
        // Completed so the TTL cleaner's status filter lets it
        // through — an in-progress recording must never be handed to
        // the cleaner, even with a past `expires_at`.
        let expired_id = "rec_ttl_expired";
        let expired_file = format!("{expired_id}.cast");
        let expired_path = dir.path().join(&expired_file);
        std::fs::write(&expired_path, b"expired").unwrap();
        storage
            .create_recording(
                expired_id,
                &session.id,
                user.id,
                80,
                24,
                &expired_file,
                Some("2020-01-01T00:00:00+00:00"),
            )
            .await
            .unwrap();
        storage
            .complete_recording(expired_id, 1000, 5, 512)
            .await
            .unwrap();

        // One permanent recording the cleaner must NOT touch.
        let keep_id = "rec_ttl_keep";
        let keep_file = format!("{keep_id}.cast");
        let keep_path = dir.path().join(&keep_file);
        std::fs::write(&keep_path, b"keep").unwrap();
        storage
            .create_recording(keep_id, &session.id, user.id, 80, 24, &keep_file, None)
            .await
            .unwrap();
        storage
            .complete_recording(keep_id, 1000, 5, 512)
            .await
            .unwrap();

        run_once(&storage, dir.path()).await;

        assert!(
            !expired_path.exists(),
            "expired .cast file should be deleted"
        );
        assert!(
            storage.get_recording(expired_id).await.unwrap().is_none(),
            "expired DB row should be deleted"
        );
        assert!(keep_path.exists(), "permanent .cast file must survive");
        assert!(
            storage.get_recording(keep_id).await.unwrap().is_some(),
            "permanent DB row must survive"
        );
    }

    /// A missing file is documented as "not an error" — the cleaner
    /// must still remove the row so it does not accumulate
    /// indefinitely after manual file deletion or a partial prior
    /// run. This is the behaviour the cleaner already relies on; the
    /// test pins it so a future refactor doesn't quietly start
    /// returning early on `NotFound`.
    #[tokio::test]
    async fn cleaner_removes_row_when_file_missing() {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let dir = tempfile::tempdir().unwrap();

        let (user, _) = storage.create_user("nofileuser", false).await.unwrap();
        let session = storage
            .create_session_with_owner(user.id, "default", InputMode::Serialized, None)
            .await
            .unwrap();

        let id = "rec_no_file";
        storage
            .create_recording(
                id,
                &session.id,
                user.id,
                80,
                24,
                &format!("{id}.cast"),
                Some("2020-01-01T00:00:00+00:00"),
            )
            .await
            .unwrap();
        // Mark completed so the cleaner's status filter lets it
        // through — active recordings are now excluded at the
        // listing layer.
        storage.complete_recording(id, 1000, 5, 512).await.unwrap();

        // No file written — run should still drop the row.
        run_once(&storage, dir.path()).await;

        assert!(storage.get_recording(id).await.unwrap().is_none());
    }
}
