/// Async task that drains a [`RecordingEvent`] channel and writes
/// asciicast v2 NDJSON to a file, flushing periodically and finalising
/// the database row on completion or error.
///
/// Callers obtain a [`RecordingSlot`] (sender + shared drop counter)
/// from [`spawn_recording_writer`] and feed events into it via
/// `slot.tx`; the PTY tap increments `slot.dropped` on every
/// back-pressured `try_send`. The returned task runs until the
/// channel closes or a [`RecordingEvent::Stop`] is received. At
/// finalisation it reads the drop counter — if any events were
/// dropped, the recording is marked `failed` rather than
/// `completed`, so the API/UI never silently serve a capture with
/// gaps in it.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use telepair_core::recording::{AsciicastHeader, RecordingEvent};
use telepair_core::storage::{SqliteStorage, Storage};
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;
use tracing::{error, warn};

/// Sender + shared drop counter handed back to the hub so the PTY
/// I/O loop (and any other tap) can report events it had to discard
/// because the writer's channel was full. The writer task holds a
/// clone of `dropped` via [`spawn_recording_writer`] and reads it
/// at finalisation: a non-zero count flips the final status from
/// `completed` to `failed`, matching the semantics of an I/O
/// failure — either way the capture is incomplete and callers must
/// not trust it. Pairing the sender and counter in one struct keeps
/// the two from drifting out of sync; every tap that uses the `tx`
/// is the same tap that must bump the counter on failure.
pub struct RecordingSlot {
    pub tx: mpsc::Sender<RecordingEvent>,
    pub dropped: Arc<AtomicU64>,
}

impl RecordingSlot {
    /// Non-blocking send that also records back-pressure. Every hub
    /// tap uses this helper so no call site can forget to bump the
    /// counter on failure — the previous `let _ = tx.try_send(...)`
    /// pattern made a lost event indistinguishable from a delivered
    /// one, and the writer then marked the recording as `completed`
    /// even when gaps existed. Dropping an event is still safe for
    /// the PTY I/O loop (the shell does not stall on a slow writer),
    /// but the slot now remembers the drop so finalisation can
    /// downgrade the status.
    pub fn try_send(&self, event: RecordingEvent) {
        if self.tx.try_send(event).is_err() {
            // `Relaxed` is sufficient: we only read this counter at
            // finalisation, under the writer's own mutable context,
            // after the channel has been closed — no concurrent
            // increment/read race exists that would benefit from a
            // stronger ordering.
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

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
/// Returns a [`RecordingSlot`] carrying the sender callers use to
/// feed [`RecordingEvent`]s AND a shared drop counter the hub's
/// taps bump whenever back-pressure forces an event to be
/// discarded. The writer task keeps its own clone of the counter so
/// it can reflect any lost events into the final DB status. The
/// task ends when either:
/// - A [`RecordingEvent::Stop`] is received, or
/// - The sender is dropped (channel closed).
///
/// On normal completion with zero drops the task calls
/// [`SqliteStorage::complete_recording`]. If **any** drops were
/// recorded during the run, the task instead calls
/// [`SqliteStorage::fail_recording`] — a capture with gaps is not
/// a "completed" capture, and the API/UI must show it as failed so
/// the operator treats it with appropriate suspicion. On an I/O
/// error the same `fail_recording` path runs **and** `on_failure`
/// executes so the hub can release the recording slot for the
/// owner to retry.
pub fn spawn_recording_writer(
    recording_id: String,
    file_path: PathBuf,
    header: AsciicastHeader,
    storage: Arc<SqliteStorage>,
    on_failure: OnFailure,
) -> RecordingSlot {
    let (tx, rx) = mpsc::channel::<RecordingEvent>(1024);
    let dropped = Arc::new(AtomicU64::new(0));
    tokio::spawn(run_writer(
        recording_id,
        file_path,
        header,
        storage,
        rx,
        on_failure,
        dropped.clone(),
    ));
    RecordingSlot { tx, dropped }
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
    dropped: Arc<AtomicU64>,
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
    //
    // All file I/O from here on uses `tokio::fs` + `tokio::io::BufWriter`
    // — the previous `std::fs` + `std::io::BufWriter` stack blocked the
    // tokio worker that happened to drive this task on every slow
    // disk op. On busy hosts that meant a single hung fsync could
    // stall every other task scheduled on the same worker (PTY
    // output fan-out, WS forwarders, HTTP handlers), not just this
    // recording. Async IO yields control back to the scheduler
    // while the disk is working, so the blast radius now stops at
    // this task.
    let file = match File::create(&file_path).await {
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
    let header_json = header_json_str(&header);
    let header_bytes = header_written_bytes(&header_json);
    if let Err(e) = write_line(&mut writer, &header_json).await {
        error!(recording_id = %recording_id, error = %e, "recording writer: failed to write header");
        abort_writer!();
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
                    if let Err(e) = writer.flush().await {
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

                if let Err(e) = write_line(&mut writer, &line).await {
                    error!(recording_id = %recording_id, error = %e, "recording writer: write failed");
                    abort_writer!();
                }

                event_count += 1;
                bytes_written += line_len;
                unflushed_bytes += line_len;

                if unflushed_bytes >= FLUSH_THRESHOLD_BYTES {
                    if let Err(e) = writer.flush().await {
                        error!(recording_id = %recording_id, error = %e, "recording writer: threshold flush failed");
                        abort_writer!();
                    }
                    unflushed_bytes = 0;
                }
            }
        }
    }

    // ── Final flush + shutdown ───────────────────────────────────────
    //
    // `shutdown().await` flushes the BufWriter AND closes the
    // underlying `tokio::fs::File`, which on most platforms calls
    // `close(2)` (not `fsync`) but also releases any kernel buffers
    // held by the tokio runtime. Skipping this used to leak the FD
    // until the task fully unwound; under high recording churn that
    // added up.
    if let Err(e) = writer.shutdown().await {
        error!(recording_id = %recording_id, error = %e, "recording writer: shutdown failed");
        abort_writer!();
    }
    drop(writer);
    // Graceful path: drop the cleanup explicitly. The hub already
    // took its slot via `stop_recording`, and dropping the closure
    // here releases the captured `Arc<SessionHub>` immediately
    // instead of waiting for the function to return.
    drop(on_failure_slot);

    // ── Complete DB row ──────────────────────────────────────────────
    let file_size = tokio::fs::metadata(&file_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(bytes_written as i64);
    let duration_ms = start.elapsed().as_millis() as i64;

    // A non-zero drop count means the PTY I/O loop had to discard
    // events because this writer's channel was full. The file is
    // still valid asciicast (we wrote everything we actually
    // received) but it has gaps the viewer cannot recover. Mark
    // the recording `failed` so the API/UI refuses to advertise it
    // as a trustworthy capture — matching how an I/O failure is
    // handled. Logging the count at `error!` level surfaces the
    // problem in operator dashboards so repeated drops can drive a
    // channel-size or writer-tuning response.
    let dropped_events = dropped.load(Ordering::Relaxed);
    if dropped_events > 0 {
        error!(
            recording_id = %recording_id,
            dropped_events,
            event_count,
            "recording writer: events were dropped under back-pressure; marking recording as failed",
        );
        mark_failed(&storage, &recording_id).await;
        return;
    }

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

// ── Helpers ────────────────────────────────────────────────────────────

fn header_json_str(header: &AsciicastHeader) -> String {
    serde_json::to_string(header).unwrap_or_default()
}

fn header_written_bytes(json: &str) -> usize {
    json.len() + 1 // +1 for the trailing newline
}

/// Write one NDJSON line (content + trailing `\n`) to the async
/// buffered writer. Issuing a single `write_all` per payload and a
/// second for the newline keeps the framing byte-for-byte identical
/// to the previous `writeln!(...)`-based emit; callers that parse
/// the file line-by-line (asciicast v2 consumers) see no change.
async fn write_line(writer: &mut BufWriter<File>, line: &str) -> std::io::Result<()> {
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    Ok(())
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

        let slot = spawn_recording_writer(
            recording.id.clone(),
            path.clone(),
            make_header(),
            storage.clone(),
            Box::new(|| {}),
        );

        slot.tx
            .send(RecordingEvent::Output(Bytes::from_static(b"hello")))
            .await
            .unwrap();
        slot.tx
            .send(RecordingEvent::Resize {
                cols: 100,
                rows: 30,
            })
            .await
            .unwrap();
        slot.tx.send(RecordingEvent::Stop).await.unwrap();
        drop(slot);

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
        let _slot = spawn_recording_writer(
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

        let slot = spawn_recording_writer(
            recording.id.clone(),
            path,
            make_header(),
            storage.clone(),
            Box::new(|| {}),
        );

        // Drop the sender without Stop — channel close should trigger graceful completion.
        drop(slot);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let row = storage.get_recording(&recording.id).await.unwrap().unwrap();
        assert_eq!(row.status, "completed");
    }

    /// Regression for "recording output is silently dropped under
    /// backpressure." Before the fix, every `try_send` failure in
    /// the PTY I/O loop was swallowed and the writer still marked
    /// the capture `completed`, so a viewer could load a recording
    /// with gaps in it and never know. The slot now carries a drop
    /// counter; at finalisation the writer downgrades the status
    /// to `failed` if any events were lost. This test bumps the
    /// counter directly (the easiest way to exercise the
    /// finalisation branch without fabricating channel
    /// back-pressure) and asserts the final status reflects the
    /// drops.
    #[tokio::test]
    async fn writer_marks_failed_when_drops_occurred() {
        let storage = make_storage().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        drop(tmp);

        let (user_id, session_id) = seed(&storage, "tester4").await;
        let recording = storage
            .create_recording(
                "rec_writer_dropped",
                &session_id,
                user_id,
                80,
                24,
                &path.to_string_lossy(),
                None,
            )
            .await
            .unwrap();

        let slot = spawn_recording_writer(
            recording.id.clone(),
            path.clone(),
            make_header(),
            storage.clone(),
            Box::new(|| {}),
        );

        // Send one real event so the file is well-formed asciicast,
        // then simulate a drop. A partial capture is still a partial
        // capture — the fix must flip to `failed` regardless of
        // whether any bytes made it through.
        slot.tx
            .send(RecordingEvent::Output(Bytes::from_static(b"partial")))
            .await
            .unwrap();
        slot.dropped
            .fetch_add(3, std::sync::atomic::Ordering::Relaxed);
        slot.tx.send(RecordingEvent::Stop).await.unwrap();
        drop(slot);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let row = storage.get_recording(&recording.id).await.unwrap().unwrap();
        assert_eq!(
            row.status, "failed",
            "writer must mark recording as failed when any events were dropped; got {row:?}",
        );
    }

    /// `RecordingSlot::try_send` is the single gate the hub taps
    /// use to report back-pressure. Pin both sides of the
    /// contract: a successful send does NOT bump the counter, and a
    /// send into a full channel DOES. Without this test a future
    /// refactor could silently strip the counter bump (the old
    /// failure mode) or double-count on success.
    #[tokio::test]
    async fn try_send_bumps_counter_only_on_backpressure() {
        // Tiny channel so we can fill it deterministically without
        // racing the writer task. No writer is spawned here — we
        // construct the slot by hand to exercise `try_send` in
        // isolation.
        let (tx, _rx) = mpsc::channel::<RecordingEvent>(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let slot = RecordingSlot {
            tx,
            dropped: dropped.clone(),
        };

        // First send fits in the buffer — counter stays at zero.
        slot.try_send(RecordingEvent::Output(Bytes::from_static(b"a")));
        assert_eq!(dropped.load(Ordering::Relaxed), 0);

        // Second send overflows the buffer (nothing drains because
        // there's no receiver task reading).
        slot.try_send(RecordingEvent::Output(Bytes::from_static(b"b")));
        assert_eq!(
            dropped.load(Ordering::Relaxed),
            1,
            "a full-channel try_send must increment the drop counter",
        );
    }
}
