/// Async task that drains a [`RecordingEvent`] channel and writes
/// asciicast v2 NDJSON to a file, flushing periodically and finalising
/// the database row on completion or error.
///
/// Callers obtain an `mpsc::Sender<RecordingEvent>` from
/// [`spawn_recording_writer`] and feed events into it; the returned
/// task is detached and runs until the channel closes or a
/// [`RecordingEvent::Stop`] is received.
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use telepair_core::recording::{AsciicastHeader, RecordingEvent};
use telepair_core::storage::{SqliteStorage, Storage};
use tokio::sync::mpsc;
use tracing::{error, warn};

/// Flush when the unflushed buffer exceeds this many bytes, even if the
/// 1-second timer has not fired yet.
const FLUSH_THRESHOLD_BYTES: usize = 64 * 1024;

/// Callback the writer invokes if it aborts due to an I/O failure.
/// Lets the caller (typically `http::start_recording`) detach the
/// dead sender from `SessionHub` so the owner can start a new
/// recording without restarting the session. `Box<dyn FnOnce>` keeps
/// the writer signature free of generics and lets the spawn site
/// capture whatever handle it needs (e.g. an `Arc<SessionHub>` plus
/// the session id) without leaking those types into this module.
pub type OnFailure = Box<dyn FnOnce() + Send + 'static>;

/// Spawn the recording writer task.
///
/// Returns an `mpsc::Sender` the caller should use to feed
/// [`RecordingEvent`]s. The task ends when either:
/// - A [`RecordingEvent::Stop`] is received, or
/// - The sender is dropped (channel closed).
///
/// On normal completion the task calls
/// [`SqliteStorage::complete_recording`]. On any I/O error it calls
/// [`SqliteStorage::fail_recording`] **and** runs `on_failure` so
/// the hub can release the recording slot for the owner to retry.
pub fn spawn_recording_writer(
    recording_id: String,
    file_path: PathBuf,
    header: AsciicastHeader,
    storage: Arc<SqliteStorage>,
    on_failure: OnFailure,
) -> mpsc::Sender<RecordingEvent> {
    let (tx, rx) = mpsc::channel::<RecordingEvent>(1024);
    tokio::spawn(run_writer(
        recording_id,
        file_path,
        header,
        storage,
        rx,
        on_failure,
    ));
    tx
}

/// Open the file, write the header, then run the event loop.
/// Returns `Ok(())` on graceful stop, `Err(())` if an I/O error forced
/// an early exit (the DB row is already marked `failed` before this
/// returns).
async fn run_writer(
    recording_id: String,
    file_path: PathBuf,
    header: AsciicastHeader,
    storage: Arc<SqliteStorage>,
    mut rx: mpsc::Receiver<RecordingEvent>,
    on_failure: OnFailure,
) {
    // Hold the cleanup callback in an Option so each failure branch
    // can `take()` and invoke it exactly once. `OnFailure` is a
    // `FnOnce`, so duplicate invocation is a compile error — `take`
    // gives the borrow checker something to work with.
    let mut on_failure_slot: Option<OnFailure> = Some(on_failure);

    // Every IO failure path runs the same three statements: mark the
    // DB row failed, run the cleanup so the hub releases its slot,
    // and bail out of the loop. A macro keeps that contract in one
    // place — adding a sixth failure branch later cannot quietly
    // forget the cleanup call.
    macro_rules! abort_writer {
        () => {{
            mark_failed(&storage, &recording_id).await;
            if let Some(cb) = on_failure_slot.take() {
                cb();
            }
            return;
        }};
    }

    // ── Open file ────────────────────────────────────────────────────
    let file = match std::fs::File::create(&file_path) {
        Ok(f) => f,
        Err(e) => {
            error!(
                recording_id = %recording_id,
                path = %file_path.display(),
                error = %e,
                "recording writer: failed to create file"
            );
            abort_writer!();
        }
    };
    let mut writer = BufWriter::new(file);

    // ── Write header ─────────────────────────────────────────────────
    let header_bytes = header_written_bytes(&header_json_str(&header));
    {
        // Scope ensures format temporaries are gone before the first await.
        let write_result = write_header(&mut writer, &header);
        if let Err(e) = write_result {
            error!(recording_id = %recording_id, error = %e, "recording writer: failed to write header");
            abort_writer!();
        }
    }

    // ── Event loop ───────────────────────────────────────────────────
    let start = Instant::now();
    let mut event_count: i64 = 0;
    let mut bytes_written: usize = header_bytes;
    let mut unflushed_bytes: usize = 0;
    let flush_interval = tokio::time::Duration::from_secs(1);

    loop {
        let recv_result = tokio::time::timeout(flush_interval, rx.recv()).await;

        match recv_result {
            // 1-second timeout — flush if anything is pending.
            Err(_timeout) => {
                if unflushed_bytes > 0 {
                    let flush_result = writer.flush();
                    if let Err(e) = flush_result {
                        error!(recording_id = %recording_id, error = %e, "recording writer: periodic flush failed");
                        abort_writer!();
                    }
                    unflushed_bytes = 0;
                }
            }

            // Channel closed (all senders dropped) — treat as graceful stop.
            Ok(None) => break,

            Ok(Some(RecordingEvent::Stop)) => break,

            Ok(Some(event)) => {
                let elapsed = start.elapsed().as_secs_f64();
                let line = event.to_asciicast_line(elapsed);
                let line_len = line.len() + 1; // +1 for the newline

                // Perform the write and capture the result before any await.
                let write_result = writeln!(writer, "{line}");
                if let Err(e) = write_result {
                    error!(recording_id = %recording_id, error = %e, "recording writer: write failed");
                    abort_writer!();
                }

                event_count += 1;
                bytes_written += line_len;
                unflushed_bytes += line_len;

                if unflushed_bytes >= FLUSH_THRESHOLD_BYTES {
                    let flush_result = writer.flush();
                    if let Err(e) = flush_result {
                        error!(recording_id = %recording_id, error = %e, "recording writer: threshold flush failed");
                        abort_writer!();
                    }
                    unflushed_bytes = 0;
                }
            }
        }
    }

    // ── Final flush ──────────────────────────────────────────────────
    {
        let flush_result = writer.flush();
        if let Err(e) = flush_result {
            error!(recording_id = %recording_id, error = %e, "recording writer: final flush failed");
            abort_writer!();
        }
    }
    drop(writer);
    // Graceful path: drop the cleanup explicitly. The hub already
    // took its slot via `stop_recording`, and dropping the closure
    // here releases the captured `Arc<SessionHub>` immediately
    // instead of waiting for the function to return.
    drop(on_failure_slot);

    // ── Complete DB row ──────────────────────────────────────────────
    let file_size = std::fs::metadata(&file_path)
        .map(|m| m.len() as i64)
        .unwrap_or(bytes_written as i64);
    let duration_ms = start.elapsed().as_millis() as i64;

    if let Err(e) = storage
        .complete_recording(&recording_id, duration_ms, event_count, file_size)
        .await
    {
        error!(
            recording_id = %recording_id,
            error = %e,
            "recording writer: complete_recording DB call failed"
        );
    }
}

// ── Helpers (sync, no await) ──────────────────────────────────────────

fn header_json_str(header: &AsciicastHeader) -> String {
    serde_json::to_string(header).unwrap_or_default()
}

fn header_written_bytes(json: &str) -> usize {
    json.len() + 1 // +1 for the trailing newline
}

fn write_header(
    writer: &mut BufWriter<std::fs::File>,
    header: &AsciicastHeader,
) -> std::io::Result<()> {
    let json = header_json_str(header);
    writeln!(writer, "{json}")
}

async fn mark_failed(storage: &Arc<SqliteStorage>, recording_id: &str) {
    if let Err(e) = storage.fail_recording(recording_id).await {
        warn!(recording_id = %recording_id, error = %e, "recording writer: fail_recording DB call failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::io::Read;
    use telepair_core::recording::RecordingEvent;
    use telepair_core::session::InputMode;
    use telepair_core::storage::Storage;

    async fn make_storage() -> Arc<SqliteStorage> {
        Arc::new(SqliteStorage::new_memory().await.unwrap())
    }

    /// Seed a user + session, returning (user_id, session_id).
    async fn seed(storage: &Arc<SqliteStorage>, name: &str) -> (uuid::Uuid, String) {
        let (user, _) = storage.create_user(name, false).await.unwrap();
        let session = storage
            .create_session_with_owner(user.id, "default", InputMode::Serialized, None)
            .await
            .unwrap();
        (user.id, session.id)
    }

    fn make_header() -> AsciicastHeader {
        AsciicastHeader {
            version: 2,
            width: 80,
            height: 24,
            timestamp: 0,
            env: Default::default(),
            telepair: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn writer_creates_file_and_completes() {
        let storage = make_storage().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        drop(tmp);

        let (user_id, session_id) = seed(&storage, "tester").await;
        let recording = storage
            .create_recording(
                "rec_writer_ok",
                &session_id,
                user_id,
                80,
                24,
                &path.to_string_lossy(),
                None,
            )
            .await
            .unwrap();

        let tx = spawn_recording_writer(
            recording.id.clone(),
            path.clone(),
            make_header(),
            storage.clone(),
            Box::new(|| {}),
        );

        tx.send(RecordingEvent::Output(Bytes::from_static(b"hello")))
            .await
            .unwrap();
        tx.send(RecordingEvent::Resize {
            cols: 100,
            rows: 30,
        })
        .await
        .unwrap();
        tx.send(RecordingEvent::Stop).await.unwrap();
        drop(tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let row = storage.get_recording(&recording.id).await.unwrap().unwrap();
        assert_eq!(row.status, "completed");
        assert_eq!(row.event_count, 2); // Output + Resize

        let mut content = String::new();
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "header + 2 events = 3 lines");
        assert!(lines[0].contains("\"version\":2"));
        assert!(lines[1].contains("\"o\""));
        assert!(lines[2].contains("\"r\""));
    }

    #[tokio::test]
    async fn writer_marks_failed_on_bad_path() {
        let storage = make_storage().await;
        let (user_id, session_id) = seed(&storage, "tester2").await;
        let bad_path = PathBuf::from("/nonexistent_dir/recording.cast");
        let recording = storage
            .create_recording(
                "rec_writer_bad",
                &session_id,
                user_id,
                80,
                24,
                &bad_path.to_string_lossy(),
                None,
            )
            .await
            .unwrap();

        let cleanup_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cleanup_flag = cleanup_called.clone();
        let _tx = spawn_recording_writer(
            recording.id.clone(),
            bad_path,
            make_header(),
            storage.clone(),
            Box::new(move || {
                cleanup_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let row = storage.get_recording(&recording.id).await.unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert!(
            cleanup_called.load(std::sync::atomic::Ordering::SeqCst),
            "on_failure must run when the writer aborts so the hub releases its slot"
        );
    }

    #[tokio::test]
    async fn writer_completes_on_channel_close() {
        let storage = make_storage().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        drop(tmp);

        let (user_id, session_id) = seed(&storage, "tester3").await;
        let recording = storage
            .create_recording(
                "rec_writer_close",
                &session_id,
                user_id,
                80,
                24,
                &path.to_string_lossy(),
                None,
            )
            .await
            .unwrap();

        let tx = spawn_recording_writer(
            recording.id.clone(),
            path,
            make_header(),
            storage.clone(),
            Box::new(|| {}),
        );

        // Drop the sender without Stop — channel close should trigger graceful completion.
        drop(tx);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let row = storage.get_recording(&recording.id).await.unwrap().unwrap();
        assert_eq!(row.status, "completed");
    }
}
