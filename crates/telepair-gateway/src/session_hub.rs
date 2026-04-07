use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use telepair_agent::pty::PtyManager;
use telepair_core::error::Result;
use telepair_core::permission::Role;
use telepair_core::protocol::{ParticipantInfo, ServerMessage};
use telepair_core::storage::{SqliteStorage, Storage};

/// Color palette for participant cursors / identifiers.
const COLORS: &[&str] = &[
    "#58a6ff", "#3fb950", "#d29922", "#f85149", "#bc8cff", "#39c5cf", "#ffa198", "#56d364",
];

/// Maximum number of bytes retained in a session's scrollback ring. A late
/// joiner gets this much recent PTY output replayed before live broadcasts
/// start streaming, which is enough for a fresh `ls` + a prompt banner but
/// small enough that a `yes` loop or `cat` of a huge file won't pin per-
/// session memory. Raise this if users complain that the replay feels too
/// short; lower it if long-lived sessions start ballooning RSS.
const SCROLLBACK_CAP_BYTES: usize = 64 * 1024;

/// A bounded ring buffer of PTY output chunks used for scrollback replay.
///
/// We keep whole `Bytes` chunks rather than copying into one contiguous
/// buffer so `push` and `snapshot` are both refcount-cheap. When the total
/// size crosses `SCROLLBACK_CAP_BYTES` the oldest chunks are dropped; this
/// can produce a slightly truncated first line on replay but never truncates
/// in the middle of a keystroke-sized chunk, which is what matters for
/// terminal escape-sequence coherence.
///
/// Guarded by `std::sync::Mutex` (not `tokio::sync::Mutex`) because the
/// critical section is pure CPU — `push`, `snapshot`, and the broadcast
/// `send` that runs alongside it never `.await`. A tokio async mutex
/// would force every PTY chunk through the async scheduler for no
/// reason; the std mutex is ~10× cheaper on the uncontended path.
struct Scrollback {
    chunks: VecDeque<Bytes>,
    total: usize,
}

impl Scrollback {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            total: 0,
        }
    }

    fn push(&mut self, bytes: Bytes) {
        self.total = self.total.saturating_add(bytes.len());
        self.chunks.push_back(bytes);
        while self.total > SCROLLBACK_CAP_BYTES {
            match self.chunks.pop_front() {
                Some(front) => self.total = self.total.saturating_sub(front.len()),
                None => break,
            }
        }
    }

    fn snapshot(&self) -> Vec<Bytes> {
        self.chunks.iter().cloned().collect()
    }
}

fn assign_color(index: usize) -> String {
    COLORS[index % COLORS.len()].to_string()
}

/// Commands sent to the PTY I/O loop.
pub enum PtyCommand {
    Input(Bytes),
    Resize(u16, u16),
}

/// Subscription bundle handed back to a WS handler attaching to a live
/// session. Named fields (not a 5-tuple) so adding a new channel
/// doesn't force every call site to re-position its destructuring and
/// the invariant of "every returned receiver lines up with `scrollback`"
/// lives in one type instead of scattered across call sites.
pub struct SessionAttachment {
    pub cmd_tx: mpsc::Sender<PtyCommand>,
    pub output_rx: broadcast::Receiver<Bytes>,
    pub collab_rx: broadcast::Receiver<ServerMessage>,
    pub shutdown_rx: broadcast::Receiver<()>,
    /// Scrollback chunks to replay before the live broadcast starts.
    /// Empty for a freshly-spawned session.
    pub scrollback: Vec<Bytes>,
}

/// Configuration for the background idle-session reaper.
///
/// `idle_timeout` is how long a `LiveSession` may sit with zero WebSocket
/// connections before the reaper tears it down (killing the PTY child and
/// marking the row closed in SQLite). `check_interval` is how often the
/// reaper wakes up to scan the session map.
#[derive(Debug, Clone, Copy)]
pub struct ReaperConfig {
    pub idle_timeout: Duration,
    pub check_interval: Duration,
}

impl Default for ReaperConfig {
    fn default() -> Self {
        Self {
            // 2 minutes of grace for reconnects / tab reloads before we
            // tear down the PTY. Short enough that abandoned sessions
            // don't leak forever, long enough that a user fumbling their
            // network doesn't lose their shell.
            idle_timeout: Duration::from_secs(120),
            check_interval: Duration::from_secs(30),
        }
    }
}

/// A running terminal session with PTY, broadcast channels, and participant tracking.
struct LiveSession {
    /// Send terminal input (or resize) to PTY via command channel
    cmd_tx: mpsc::Sender<PtyCommand>,
    /// Subscribe to PTY output. Uses `Bytes` so broadcast cloning is a cheap
    /// refcount bump per subscriber instead of a per-chunk `Vec<u8>` copy.
    output_tx: broadcast::Sender<Bytes>,
    /// Broadcast collaboration messages (PeerJoined, PeerLeft, PeerChat, PeerCursor).
    collab_tx: broadcast::Sender<ServerMessage>,
    /// Signal to all connected WS handlers that this session is being force-stopped
    shutdown_tx: broadcast::Sender<()>,
    /// Ring buffer of recent PTY output. The PTY task locks this before each
    /// broadcast so a new subscriber can atomically take a snapshot + the
    /// broadcast receiver and guarantee no duplication and no gap: any chunk
    /// that arrives while the snapshot is being taken is serialized behind
    /// the lock and will land in the subscriber's broadcast buffer instead.
    scrollback: Arc<Mutex<Scrollback>>,
    /// Currently connected participants, keyed by user_id. A single user may
    /// open multiple tabs/devices; the map stores one canonical record and
    /// `connections` below counts how many live WS handlers are attached.
    participants: HashMap<Uuid, ParticipantInfo>,
    /// WS-handler reference count per user_id. Invariant: a `user_id` is in
    /// `participants` iff `connections[user_id] > 0`. Without this counter a
    /// user's second tab closing would broadcast `PeerLeft` and wipe the
    /// in-memory entry even though another tab is still connected.
    connections: HashMap<Uuid, usize>,
    /// When the last WS connection dropped. `None` while at least one
    /// connection is attached; set to `Some(Instant::now())` on the
    /// transition to 0 connections and cleared back to `None` on reconnect.
    /// The background reaper uses this to decide when to tear down the
    /// session — holding cmd_tx in the hub alone keeps the PTY loop alive
    /// forever, so we need an explicit idle tracker.
    idle_since: Option<Instant>,
    /// Monotonic counter for color assignment
    color_counter: usize,
}

pub struct SessionHub {
    sessions: Arc<RwLock<HashMap<String, LiveSession>>>,
    storage: Arc<SqliteStorage>,
}

impl SessionHub {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            storage,
        }
    }

    /// Start or join a live session. Checks under read lock first, spawns
    /// PTY without holding any lock, then inserts under write lock with a
    /// TOCTOU re-check.
    pub async fn start_or_join(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<SessionAttachment> {
        // Fast path: session already running — subscribe under read lock.
        // `attach_to` grabs the scrollback lock, calls
        // `output_tx.subscribe()` inside it, and only then snapshots, so the
        // returned receiver and snapshot line up without gap or duplication.
        {
            let sessions = self.sessions.read().await;
            if let Some(live) = sessions.get(session_id) {
                return Ok(Self::attach_to(live));
            }
        }

        // Spawn PTY without holding any lock (fork/exec can be slow)
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut pty = PtyManager::spawn_command(command, &args_ref, cols, rows, env)?;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(256);
        let (output_tx, output_rx) = broadcast::channel::<Bytes>(256);
        let (collab_tx, collab_rx) = broadcast::channel::<ServerMessage>(64);
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
        let scrollback = Arc::new(Mutex::new(Scrollback::new()));

        // Re-acquire write lock and check for TOCTOU race
        let mut sessions = self.sessions.write().await;
        if let Some(live) = sessions.get(session_id) {
            // Another task spawned the session while we were spawning — use theirs,
            // our PTY will be dropped and the child process cleaned up
            drop(pty);
            return Ok(Self::attach_to(live));
        }

        let output_tx_clone = output_tx.clone();
        let session_id_owned = session_id.to_string();
        let sessions_arc = self.sessions.clone();
        let storage_clone = self.storage.clone();
        let scrollback_for_pty = scrollback.clone();

        // PTY I/O loop -- single task owns the PtyManager
        tokio::spawn(async move {
            loop {
                enum Action {
                    Output(Option<Bytes>),
                    Command(Option<PtyCommand>),
                }

                let action = tokio::select! {
                    data = pty.read() => Action::Output(data),
                    cmd = cmd_rx.recv() => Action::Command(cmd),
                };

                match action {
                    Action::Output(Some(bytes)) => {
                        // Record into the scrollback ring AND broadcast
                        // under the same lock. Holding the scrollback
                        // mutex across both is what gives us the "every
                        // chunk is in the snapshot OR in the broadcast,
                        // never both, never neither" invariant:
                        //
                        //   - A subscriber that grabs the lock first
                        //     takes a snapshot that does NOT contain
                        //     this chunk, then subscribes, then releases.
                        //     This task then locks, pushes, broadcasts —
                        //     the subscriber receives the chunk over the
                        //     broadcast. No gap, no duplicate.
                        //
                        //   - A subscriber that arrives while this task
                        //     holds the lock waits until push+broadcast
                        //     complete. Its snapshot will contain the
                        //     chunk, and because the chunk was already
                        //     broadcast before the subscriber called
                        //     `subscribe()`, it will NOT be re-delivered.
                        //
                        // If we released the lock between push and
                        // broadcast, a subscriber could slip in and
                        // snapshot the chunk *and* subscribe before the
                        // broadcast fires, causing a duplicate.
                        // std::sync::Mutex: push + send are both CPU-only
                        // and never .await. Holding the lock across
                        // `send` is what gives attach_to's snapshot the
                        // "every chunk is in the snapshot OR delivered,
                        // never both" invariant (see attach_to's doc).
                        let mut sb = scrollback_for_pty
                            .lock()
                            .expect("scrollback mutex poisoned");
                        sb.push(bytes.clone());
                        // Already a refcounted Bytes from the PTY reader;
                        // every subscriber clone is an Arc bump, not a copy.
                        let _ = output_tx_clone.send(bytes);
                        drop(sb);
                    }
                    Action::Output(None) => {
                        tracing::info!(session = %session_id_owned, "PTY process exited");
                        break;
                    }
                    Action::Command(Some(PtyCommand::Input(data))) => {
                        if pty.write(data).await.is_err() {
                            break;
                        }
                    }
                    Action::Command(Some(PtyCommand::Resize(cols, rows))) => {
                        let _ = pty.resize(cols, rows);
                    }
                    Action::Command(None) => {
                        tracing::info!(session = %session_id_owned, "all clients disconnected");
                        break;
                    }
                }
            }

            // Cleanup: remove from in-memory map and close in DB.
            // `SessionNotFound` here is the *expected* outcome when the
            // HTTP close path or the reaper already closed the DB row;
            // logging that as a warning spammed the logs on every
            // deliberate `DELETE /api/sessions/{id}` call. Reserve the
            // warn! for unexpected failures (lock contention, DB error,
            // etc.) and drop the "already closed" case to debug so it
            // stays out of production dashboards.
            sessions_arc.write().await.remove(&session_id_owned);
            match storage_clone.close_session(&session_id_owned).await {
                Ok(()) => {}
                Err(telepair_core::error::Error::SessionNotFound(_)) => {
                    tracing::debug!(
                        session = %session_id_owned,
                        "PTY cleanup: session already closed in DB, nothing to do",
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        session = %session_id_owned,
                        "failed to close session in DB: {e}",
                    );
                }
            }
        });

        let live = LiveSession {
            cmd_tx: cmd_tx.clone(),
            output_tx: output_tx.clone(),
            collab_tx: collab_tx.clone(),
            shutdown_tx,
            scrollback: scrollback.clone(),
            participants: HashMap::new(),
            connections: HashMap::new(),
            // Start the clock on creation: if nobody ever calls
            // `add_participant` after start_or_join (e.g. the WS handler
            // errors out between spawning the PTY and attaching) the
            // reaper will clean this up instead of leaking a shell.
            idle_since: Some(Instant::now()),
            color_counter: 0,
        };
        sessions.insert(session_id.to_string(), live);

        // Brand-new session: no scrollback yet, and nothing can have been
        // broadcast on `output_rx` since we just created it — reuse the
        // original channels instead of re-subscribing.
        Ok(SessionAttachment {
            cmd_tx,
            output_rx,
            collab_rx,
            shutdown_rx,
            scrollback: Vec::new(),
        })
    }

    /// Build a `SessionAttachment` for a subscriber that's joining an
    /// already-live session. Atomically subscribes to PTY output AND
    /// snapshots scrollback under the scrollback lock so the two line up
    /// without gap or duplication:
    ///
    ///   - Any chunk pushed before we acquired the lock is in the
    ///     snapshot. We were not subscribed when it was broadcast, so
    ///     the receiver will not re-deliver it.
    ///   - Any chunk pushed after we release the lock will be delivered
    ///     via the receiver, because we already subscribed before the
    ///     writer could acquire the lock again.
    ///
    /// If we called `subscribe()` after releasing the lock, a chunk
    /// could slip in during that gap and be lost entirely — not in the
    /// snapshot, and broadcast before the receiver existed. The `cmd_tx`,
    /// `collab_tx`, and `shutdown_tx` clones don't need the lock and are
    /// taken after releasing it to keep the critical section tight.
    fn attach_to(live: &LiveSession) -> SessionAttachment {
        // std::sync::Mutex — critical section is purely CPU (subscribe
        // + snapshot) so no async lock needed. Never held across .await.
        let (output_rx, scrollback) = {
            let sb = live.scrollback.lock().expect("scrollback mutex poisoned");
            let output_rx = live.output_tx.subscribe();
            let scrollback = sb.snapshot();
            (output_rx, scrollback)
        };
        SessionAttachment {
            cmd_tx: live.cmd_tx.clone(),
            output_rx,
            collab_rx: live.collab_tx.subscribe(),
            shutdown_rx: live.shutdown_tx.subscribe(),
            scrollback,
        }
    }

    /// Force-stop a live session by signalling all connected WS handlers,
    /// then removing it from the map.
    pub async fn stop_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(live) = sessions.get(session_id) {
            let _ = live.shutdown_tx.send(());
        }
        sessions.remove(session_id);
    }

    /// Register a WS connection for a participant AND return the full
    /// participant snapshot under the same write lock, so the caller can
    /// deliver a `SessionState` containing the joining user without a
    /// second round-trip through `get_participants`. The first connection
    /// for a given `user_id` broadcasts `PeerJoined` and assigns a color;
    /// subsequent connections (e.g. a second tab) just bump the refcount
    /// and reuse the existing participant record. Returns `None` if the
    /// session isn't in the hub.
    pub async fn add_participant_and_snapshot(
        &self,
        session_id: &str,
        user_id: Uuid,
        name: String,
        role: Role,
    ) -> Option<Vec<ParticipantInfo>> {
        let mut sessions = self.sessions.write().await;
        let live = sessions.get_mut(session_id)?;

        // Any successful attach clears the idle clock. Do this before the
        // refcount bump so a reconnect during the reaper's grace period
        // always wins against a pending sweep.
        live.idle_since = None;

        let count = live.connections.entry(user_id).or_insert(0);
        *count += 1;
        let first_connection = *count == 1;

        if first_connection {
            let color = assign_color(live.color_counter);
            live.color_counter += 1;

            live.participants.insert(
                user_id,
                ParticipantInfo {
                    user_id,
                    name: name.clone(),
                    role,
                    color: color.clone(),
                },
            );

            // Broadcast PeerJoined to all collab subscribers
            let _ = live.collab_tx.send(ServerMessage::PeerJoined {
                user_id,
                name,
                role,
                color,
            });
        }
        // Else: additional connection from an already-joined user — don't
        // re-broadcast PeerJoined or reassign a color; the existing
        // participant record stays authoritative.

        Some(live.participants.values().cloned().collect())
    }

    /// Unregister a WS connection. Only the final connection for a `user_id`
    /// removes the participant record and broadcasts `PeerLeft`. Returns
    /// `true` when the caller was that final connection — callers use this
    /// to decide whether to update the DB `left_at` column, which must only
    /// move when the user really left all their tabs.
    pub async fn remove_participant(&self, session_id: &str, user_id: Uuid) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(live) = sessions.get_mut(session_id) else {
            return false;
        };

        let Some(count) = live.connections.get_mut(&user_id) else {
            return false;
        };
        if *count > 1 {
            *count -= 1;
            return false;
        }

        // Last connection for this user — tear down the participant record.
        live.connections.remove(&user_id);
        if live.participants.remove(&user_id).is_some() {
            let _ = live.collab_tx.send(ServerMessage::PeerLeft { user_id });
        }

        // If this was the last connection for the whole session, start
        // the idle clock so the reaper can collect it after the grace
        // period. A new `add_participant` call will clear this again.
        if live.connections.is_empty() {
            live.idle_since = Some(Instant::now());
        }

        true
    }

    /// Broadcast an arbitrary collaboration message to all subscribers in a session.
    pub async fn broadcast_collab(&self, session_id: &str, msg: ServerMessage) {
        let sessions = self.sessions.read().await;
        if let Some(live) = sessions.get(session_id) {
            let _ = live.collab_tx.send(msg);
        }
    }

    /// Spawn a background task that periodically reaps sessions which
    /// have had zero WS connections for longer than `config.idle_timeout`.
    ///
    /// This is the load-bearing piece of the "all clients left → shell
    /// goes away" story. The hub keeps a clone of `cmd_tx` inside
    /// `LiveSession` so it can hand it out to late joiners, which means
    /// the PTY I/O loop's `cmd_rx.recv()` never returns `None` on its
    /// own. Instead, the reaper removes the whole `LiveSession` from the
    /// map; dropping it drops the last `cmd_tx` and the PTY loop exits
    /// cleanly via its `Action::Command(None)` branch, which kills the
    /// child process (via `PtyManager::Drop`) and marks the row closed.
    ///
    /// Returns the `JoinHandle` so callers can abort the reaper during
    /// graceful shutdown (or just drop it to let it run for the process
    /// lifetime).
    pub fn spawn_reaper(self: &Arc<Self>, config: ReaperConfig) -> JoinHandle<()> {
        let sessions = self.sessions.clone();
        let storage = self.storage.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(config.check_interval);
            // Skip missed ticks instead of bursting — if the reaper was
            // delayed by a heavy write lock holder we don't want to scan
            // the map back-to-back.
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            // `interval` fires immediately on first tick; drop that one
            // so brand-new sessions aren't evaluated before anybody had
            // a chance to attach.
            ticker.tick().await;

            loop {
                ticker.tick().await;

                // Collect under a read lock so we don't block input
                // handling on every sweep. Only upgrade to write when we
                // have something to kill.
                let to_reap: Vec<String> = {
                    let guard = sessions.read().await;
                    guard
                        .iter()
                        .filter_map(|(id, live)| {
                            let idle_since = live.idle_since?;
                            (idle_since.elapsed() >= config.idle_timeout).then(|| id.clone())
                        })
                        .collect()
                };

                if to_reap.is_empty() {
                    continue;
                }

                // Re-check each candidate under the write lock in case a
                // reconnect raced in between the read-lock scan and now.
                let reaped: Vec<String> = {
                    let mut guard = sessions.write().await;
                    to_reap
                        .into_iter()
                        .filter(|id| {
                            let Some(live) = guard.get(id) else {
                                return false;
                            };
                            let Some(idle_since) = live.idle_since else {
                                return false;
                            };
                            if idle_since.elapsed() < config.idle_timeout {
                                return false;
                            }
                            // Remove now while we hold the lock. Dropping
                            // `live` drops the last cmd_tx and the PTY
                            // loop will notice and exit.
                            guard.remove(id);
                            true
                        })
                        .collect()
                };

                for id in reaped {
                    tracing::info!(
                        session = %id,
                        idle_secs = config.idle_timeout.as_secs(),
                        "reaped idle session"
                    );
                    // Best-effort close in the DB. The PTY loop's
                    // cleanup path will also call this, but whichever
                    // lands first wins — `close_session` is idempotent
                    // enough (the second call returns SessionNotFound
                    // because the row is already closed), and we'd
                    // rather double-log than leak an open row.
                    if let Err(e) = storage.close_session(&id).await {
                        tracing::debug!(session = %id, "reaper close_session: {e}");
                    }
                }
            }
        })
    }
}
