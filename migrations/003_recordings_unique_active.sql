-- Enforce one active recording per session at the storage layer.
--
-- The previous design relied on an application-level "find then insert"
-- check in `RecordingService::create_recording`. Two concurrent
-- POST /api/sessions/:id/recording/start requests could both pass that
-- check and insert duplicate rows; the loser would then race the hub
-- slot grab, get marked `failed`, and leave behind an orphan `.cast`
-- file (the writer task spawns before the slot grab and writes the
-- header eagerly). The DB-level unique partial index closes that race
-- by serializing the second insert into a `SQLITE_CONSTRAINT_UNIQUE`
-- that the storage layer translates back to `Error::Conflict (409)`.
--
-- Step 1 — clear orphans before the index lands. Any pre-existing
-- `status = 'recording'` row at this point is by definition stale: the
-- only thing that keeps such a row legitimate is a live in-process
-- writer task, and we are running inside startup migration code, so
-- the process just booted and no writer can exist yet. A pre-fix DB
-- may also hold duplicate rows from the race above; without this sweep
-- the CREATE UNIQUE INDEX below would fail on those DBs.
UPDATE recordings
SET status = 'failed',
    completed_at = COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
WHERE status = 'recording';

-- Step 2 — partial unique index. SQLite serializes writes per
-- connection, so a second concurrent INSERT with the same session_id
-- + status='recording' fails immediately and the storage layer maps
-- the violation to `Error::Conflict`.
CREATE UNIQUE INDEX IF NOT EXISTS idx_recordings_one_active_per_session
    ON recordings(session_id) WHERE status = 'recording';
