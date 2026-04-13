use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use telepair_agent::pty::PtyManager;
use telepair_control::session_service::SessionService;
use telepair_core::error::Result;
use telepair_core::permission::Role;
use telepair_core::protocol::{ParticipantInfo, ServerMessage};
use telepair_core::session::CloseReason;

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

/// How long a [`SessionEntry::Pending`] reservation may sit in the hub
/// without being upgraded to [`SessionEntry::Live`] before it is GCed.
///
/// The reservation exists to bridge the create-session → WS-attach gap
/// (see [`SessionHub::reserve_target`]). 60 seconds comfortably covers
/// a slow client doing TLS + WS handshake on a flaky network plus a
/// browser tab that lost focus mid-create. If a client really cannot
/// attach in 60 s, the idle reaper will clean up the DB row at its
/// 120 s grace anyway, so the two timelines never collide.
const DEFAULT_PENDING_ATTACH_TTL: Duration = Duration::from_secs(60);

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

/// Parameters the hub needs to spawn a brand-new PTY for a session.
///
/// Packed into a struct so [`SessionHub::start_or_join`] keeps a
/// short, readable signature — the alternative (six positional
/// parameters: command, args, env, cols, rows, plus the two
/// identity strings) trips clippy's `too_many_arguments` lint and,
/// more importantly, makes call sites hard to read. Every caller
/// resolves these fields in one shot from the live `TargetEngine`
/// anyway, so grouping them matches the natural flow.
pub struct PtyLaunch<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub env: &'a HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
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

/// One slot in the hub's session map. The hub used to be a flat
/// `HashMap<String, LiveSession>` but that map was blind to a session
/// during the gap between `POST /api/sessions` (DB row inserted) and
/// the client's WS handshake (`start_or_join` actually inserts the
/// `LiveSession`). A concurrent `targets` reload that landed inside
/// that gap would see zero "live" sessions for the brand-new target
/// and happily drop it, orphaning the about-to-attach session with
/// `cleanup_orphan_session` stamping `CloseReason::Error` on the row.
///
/// Adding a `Pending` variant lets the create-session HTTP path
/// reserve the target slot the instant the DB row commits, so the
/// reload guard sees it as still in use until the WS attach upgrades
/// it to `Live`. The `Pending` variant carries only the bits the
/// guard needs (`target_name`) plus a wall-clock stamp so stale
/// reservations (client minted a session but never connected) can be
/// GCed after [`DEFAULT_PENDING_ATTACH_TTL`].
enum SessionEntry {
    /// PTY is running and at least one WS handler attached at some
    /// point. Standard happy-path entry.
    Live(LiveSession),
    /// DB row exists, client has not yet attached. Counted by
    /// [`SessionHub::count_live_sessions_per_target`] so the reload
    /// guard treats the target as still referenced; carries no
    /// channels because there is no PTY yet. Replaced by `Live` when
    /// `start_or_join` actually spawns the shell, or removed by lazy
    /// GC after the TTL expires.
    Pending {
        target_name: String,
        reserved_at: Instant,
    },
}

impl SessionEntry {
    /// Target name this entry references, regardless of variant.
    /// Used by `count_live_sessions_per_target` so the reload guard
    /// can fold both Live and Pending into the same per-target
    /// counter.
    fn target_name(&self) -> &str {
        match self {
            Self::Live(live) => &live.target_name,
            Self::Pending { target_name, .. } => target_name,
        }
    }
}

/// A running terminal session with PTY, broadcast channels, and participant tracking.
struct LiveSession {
    /// The `targets.yaml` entry this live session was spawned from.
    /// Stored here so the admin reload handler can count "live
    /// sessions per target" directly off the hub's in-memory map
    /// instead of relying on the DB `sessions.target_name` column,
    /// which can be stale (row still `status='active'` while the
    /// PTY has already exited and is waiting on the reaper). The
    /// hub is the source of truth for "is this target actually in
    /// use right now" — the reload guard uses that to decide
    /// whether dropping a target would wedge a live shell.
    target_name: String,
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
    sessions: Arc<RwLock<HashMap<String, SessionEntry>>>,
    /// Handle back to the control layer so PTY cleanup and the reaper
    /// can close sessions through the service (and pick up any
    /// service-level side effects like future audit events) instead
    /// of reaching past it into raw storage.
    session_service: Arc<SessionService>,
    /// TTL for [`SessionEntry::Pending`] reservations before lazy GC
    /// drops them. Injectable so tests can use a short TTL instead of
    /// waiting 60 real seconds for expiry assertions. Production
    /// wiring uses [`SessionHub::new`] which picks
    /// [`DEFAULT_PENDING_ATTACH_TTL`].
    pending_attach_ttl: Duration,
}

impl SessionHub {
    pub fn new(session_service: Arc<SessionService>) -> Self {
        Self::with_pending_attach_ttl(session_service, DEFAULT_PENDING_ATTACH_TTL)
    }

    /// Construct a hub with a custom pending-reservation TTL. The
    /// only non-test caller is [`SessionHub::new`]; tests use this to
    /// drive the lazy GC assertion in `count_live_sessions_per_target`
    /// without sleeping for the full default TTL.
    pub fn with_pending_attach_ttl(
        session_service: Arc<SessionService>,
        pending_attach_ttl: Duration,
    ) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_service,
            pending_attach_ttl,
        }
    }

    /// Start or join a live session. Checks under read lock first, spawns
    /// PTY without holding any lock, then inserts under write lock with a
    /// TOCTOU re-check.
    ///
    /// `target_name` must match the `targets.yaml` entry the caller
    /// resolved `launch.command`/`launch.args`/`launch.env` from —
    /// it is stored inside the resulting [`LiveSession`] so the
    /// admin reload guard can count live sessions per target without
    /// touching the DB.
    pub async fn start_or_join(
        &self,
        session_id: &str,
        target_name: &str,
        launch: PtyLaunch<'_>,
    ) -> Result<SessionAttachment> {
        let PtyLaunch {
            command,
            args,
            env,
            cols,
            rows,
        } = launch;
        // Fast path: session already Live — subscribe under read lock.
        // A Pending reservation does NOT take this path; it means no
        // PTY has been spawned yet, so we fall through to the slow
        // path below and upgrade the entry under the write lock.
        // `attach_to` grabs the scrollback lock, calls
        // `output_tx.subscribe()` inside it, and only then snapshots, so the
        // returned receiver and snapshot line up without gap or duplication.
        {
            let sessions = self.sessions.read().await;
            if let Some(SessionEntry::Live(live)) = sessions.get(session_id) {
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

        // Re-acquire write lock and check for TOCTOU race. Three cases:
        //   - Live already present: another task won; use theirs.
        //   - Pending present: our create-session-path reservation (or
        //     a stale one from before a restart) — we'll overwrite it
        //     with the fresh Live entry below.
        //   - Absent: freshly insert Live.
        let mut sessions = self.sessions.write().await;
        if let Some(SessionEntry::Live(live)) = sessions.get(session_id) {
            // Another task spawned the session while we were spawning — use theirs,
            // our PTY will be dropped and the child process cleaned up
            drop(pty);
            return Ok(Self::attach_to(live));
        }

        let output_tx_clone = output_tx.clone();
        let session_id_owned = session_id.to_string();
        let sessions_arc = self.sessions.clone();
        let session_service_clone = self.session_service.clone();
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
            // PTY-loop cleanup is the belt-and-suspenders close path:
            // the HTTP handler or the reaper usually reaches the row
            // first (carrying `Owner` or `Reaper`) and this call then
            // no-ops on `SessionNotFound`. When it DOES get there
            // first (e.g. the PTY exited on its own), `Reaper` is the
            // closest fit — the session went idle and a sweeper
            // reclaimed it. Adding a dedicated variant would double
            // the UI chip count without new signal.
            match session_service_clone
                .close_session(&session_id_owned, CloseReason::Reaper, None)
                .await
            {
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
            target_name: target_name.to_string(),
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
        // `insert` overwrites a stale `Pending` reservation in place
        // — that is the load-bearing semantic for closing the
        // create-session → WS-attach gap. If a Pending exists, this
        // call is the WS-attach upgrading it to Live; if no entry
        // exists (e.g. a session created before reservation wiring),
        // this is just a fresh insert.
        sessions.insert(session_id.to_string(), SessionEntry::Live(live));

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
    /// then removing it from the map. A `Pending` entry has no WS
    /// handlers to signal, but is still removed so a stop call drops
    /// any reservation as well.
    pub async fn stop_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(SessionEntry::Live(live)) = sessions.get(session_id) {
            let _ = live.shutdown_tx.send(());
        }
        sessions.remove(session_id);
    }

    /// Count how many live sessions reference each `targets.yaml`
    /// entry, keyed by the target name. Targets with zero live
    /// sessions are omitted from the map (callers should treat
    /// absence as zero).
    ///
    /// This is the in-memory counterpart to
    /// [`telepair_control::session_service::SessionService::active_session_counts_per_target`]:
    /// the DB count can briefly report `active` rows that no longer
    /// have a live PTY (the reaper hasn't run yet, or the PTY
    /// cleanup path hasn't landed its close). The hub map, on the
    /// other hand, is exactly the set of running shells **plus**
    /// pending reservations from sessions that have been created in
    /// the DB but whose WS handler has not yet attached. Both must
    /// count: the reaper-stale case is the reason we don't fall back
    /// to the DB, and the pending case is the reason this method
    /// folds in `SessionEntry::Pending` — without it, an admin
    /// reload landing during the create-session → WS-attach gap
    /// would drop the brand-new target and orphan the session.
    ///
    /// This walk also lazily GCs pending reservations older than
    /// `pending_attach_ttl`, so a client that fetched a session and
    /// then never connected does not block reloads forever. Done
    /// here under the write lock so we don't need a separate
    /// background sweeper for what is effectively a 60-second
    /// timeout. Cheap: one walk of a small HashMap.
    pub async fn count_live_sessions_per_target(&self) -> HashMap<String, u32> {
        let mut sessions = self.sessions.write().await;
        let now = Instant::now();
        let pending_ttl = self.pending_attach_ttl;
        sessions.retain(|_, entry| match entry {
            SessionEntry::Pending { reserved_at, .. } => {
                now.duration_since(*reserved_at) < pending_ttl
            }
            SessionEntry::Live(_) => true,
        });
        let mut counts: HashMap<String, u32> = HashMap::new();
        for entry in sessions.values() {
            *counts.entry(entry.target_name().to_string()).or_insert(0) += 1;
        }
        counts
    }

    /// Reserve a hub slot for a session whose DB row was just
    /// committed but whose WS handler has not yet attached. Without
    /// this, [`Self::count_live_sessions_per_target`] would report
    /// zero live sessions for the new target, and a concurrent
    /// `targets` reload could drop it — the next WS attach would
    /// then fail target resolution and `cleanup_orphan_session`
    /// would stamp the row `Error`.
    ///
    /// Idempotent: a no-op if a Live entry already exists for this
    /// id (the WS attach beat us to the lock — nothing to do, the
    /// Live entry is the source of truth) or if a Pending already
    /// exists with the same name (double-create defensive). The
    /// reservation is upgraded to `Live` by [`Self::start_or_join`]
    /// when the WS handler arrives, or GCed by
    /// [`Self::count_live_sessions_per_target`] /
    /// [`Self::spawn_reaper`] after `pending_attach_ttl`.
    pub async fn reserve_target(&self, session_id: &str, target_name: &str) {
        let mut sessions = self.sessions.write().await;
        // `or_insert_with` is the "leave Live alone, leave existing
        // Pending alone" rule in two words.
        sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionEntry::Pending {
                target_name: target_name.to_string(),
                reserved_at: Instant::now(),
            });
    }

    /// Drop a pending reservation. Used by error paths in the
    /// HTTP/WS layer that minted a reservation but then could not
    /// follow through (e.g. PTY spawn failed). Strictly a Pending →
    /// removed transition: a Live entry is left untouched so a stale
    /// release call from a slow error path cannot evict an
    /// already-attached session by accident.
    pub async fn release_reservation(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if matches!(sessions.get(session_id), Some(SessionEntry::Pending { .. })) {
            sessions.remove(session_id);
        }
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
        // Pending → None: a participant cannot be added to a slot
        // that has no PTY yet. Reaching this branch means the WS
        // handler raced past `start_or_join` without upgrading the
        // entry, which is a bug — the caller's `unwrap_or_else`
        // already logs and falls back gracefully.
        let live = match sessions.get_mut(session_id)? {
            SessionEntry::Live(live) => live,
            SessionEntry::Pending { .. } => return None,
        };

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
        // A Pending entry has no participants by construction.
        let Some(SessionEntry::Live(live)) = sessions.get_mut(session_id) else {
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
        if let Some(SessionEntry::Live(live)) = sessions.get(session_id) {
            let _ = live.collab_tx.send(msg);
        }
    }

    /// Update a participant's role in a live session. Mutates the
    /// in-memory participant map and broadcasts `PeerRoleChanged` so
    /// every connected client updates its participant list and
    /// re-evaluates input permissions. Does **not** persist to the
    /// database — the caller is responsible for that (see
    /// `update_participant_role` in `http.rs`). Returns `true` if the
    /// role was actually changed, `false` if the participant wasn't
    /// found in the live session or the role was already the requested
    /// value.
    pub async fn update_participant_role(
        &self,
        session_id: &str,
        target_user_id: Uuid,
        new_role: Role,
    ) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(SessionEntry::Live(live)) = sessions.get_mut(session_id) else {
            return false;
        };
        let Some(info) = live.participants.get_mut(&target_user_id) else {
            return false;
        };
        if info.role == new_role {
            return false;
        }
        info.role = new_role;
        let _ = live.collab_tx.send(ServerMessage::PeerRoleChanged {
            user_id: target_user_id,
            new_role,
        });
        true
    }

    /// Return the authoritative role for a participant in a live session.
    /// Used by the WS handler to re-sync after a broadcast lag.
    pub async fn get_participant_role(&self, session_id: &str, user_id: Uuid) -> Option<Role> {
        let sessions = self.sessions.read().await;
        match sessions.get(session_id)? {
            SessionEntry::Live(live) => live.participants.get(&user_id).map(|p| p.role),
            SessionEntry::Pending { .. } => None,
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
        let session_service = self.session_service.clone();
        let pending_attach_ttl = self.pending_attach_ttl;
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
                // have something to kill. Pending entries are skipped
                // here — they have no PTY and are GCed by the
                // dedicated retain pass below (and lazily in
                // `count_live_sessions_per_target`).
                let to_reap: Vec<String> = {
                    let guard = sessions.read().await;
                    guard
                        .iter()
                        .filter_map(|(id, entry)| {
                            let SessionEntry::Live(live) = entry else {
                                return None;
                            };
                            let idle_since = live.idle_since?;
                            (idle_since.elapsed() >= config.idle_timeout).then(|| id.clone())
                        })
                        .collect()
                };

                // GC expired pending reservations on every tick so an
                // admin reload doesn't have to be the only thing that
                // can drop them. Folded together with the Live reap
                // path so we take the write lock at most once per
                // sweep.
                if to_reap.is_empty() {
                    let mut guard = sessions.write().await;
                    let now = Instant::now();
                    guard.retain(|_, entry| match entry {
                        SessionEntry::Pending { reserved_at, .. } => {
                            now.duration_since(*reserved_at) < pending_attach_ttl
                        }
                        SessionEntry::Live(_) => true,
                    });
                    continue;
                }

                // Re-check each candidate under the write lock in case a
                // reconnect raced in between the read-lock scan and now.
                let reaped: Vec<String> = {
                    let mut guard = sessions.write().await;
                    // Sweep expired pending reservations under the same
                    // write lock for free.
                    let now_pending = Instant::now();
                    guard.retain(|_, entry| match entry {
                        SessionEntry::Pending { reserved_at, .. } => {
                            now_pending.duration_since(*reserved_at) < pending_attach_ttl
                        }
                        SessionEntry::Live(_) => true,
                    });
                    to_reap
                        .into_iter()
                        .filter(|id| {
                            let Some(SessionEntry::Live(live)) = guard.get(id) else {
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
                    if let Err(e) = session_service
                        .close_session(&id, CloseReason::Reaper, None)
                        .await
                    {
                        tracing::debug!(session = %id, "reaper close_session: {e}");
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    //! Pending-reservation state-machine tests.
    //!
    //! These exist as a unit suite (rather than living next to the
    //! gateway integration tests) because the reservation lifecycle
    //! is purely a hub-internal invariant — `reserve_target` →
    //! counted → `release_reservation` / lazy GC — and exercising it
    //! end-to-end through HTTP would tie the assertion to a lot of
    //! incidental state. The end-to-end "create_session beats reload"
    //! regression lives in `tests/admin_targets_test.rs`; this suite
    //! pins the underlying state machine so a future refactor can't
    //! silently break the GC or the Live/Pending precedence.
    use super::*;
    use telepair_control::session_service::SessionService;
    use telepair_core::audit::AuditSink;
    use telepair_core::storage::SqliteStorage;

    /// Build a fresh hub with the given pending TTL on top of an
    /// in-memory SQLite. The `SessionService` is real but the tests
    /// in this module never call into it — it exists only to satisfy
    /// `SessionHub::with_pending_attach_ttl`'s signature.
    async fn fresh_hub(pending_ttl: Duration) -> SessionHub {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let audit = Arc::new(AuditSink::new(storage.clone()));
        let sessions = Arc::new(SessionService::new(storage, audit));
        SessionHub::with_pending_attach_ttl(sessions, pending_ttl)
    }

    #[tokio::test]
    async fn reservation_is_counted_per_target() {
        // The whole point of `reserve_target`: a brand-new session
        // whose WS handler has not yet attached must still appear in
        // the reload guard's per-target counter, otherwise a
        // concurrent targets reload will drop the target out from
        // under it. We assert the counter sees both Pending entries
        // and groups them by target name (two reservations on the
        // same target collapse to count = 2).
        let hub = fresh_hub(Duration::from_secs(60)).await;
        hub.reserve_target("sess-a", "alpha").await;
        hub.reserve_target("sess-b", "alpha").await;
        hub.reserve_target("sess-c", "beta").await;

        let counts = hub.count_live_sessions_per_target().await;
        assert_eq!(counts.get("alpha"), Some(&2));
        assert_eq!(counts.get("beta"), Some(&1));
    }

    #[tokio::test]
    async fn reservation_is_lazily_gced_after_ttl() {
        // The reservation TTL exists so a client that fetches a
        // session-id and then never attaches doesn't block reloads
        // forever. We use a very short TTL plus a sleep that
        // comfortably exceeds it, then assert that the next
        // `count_live_sessions_per_target` call sees the slot as
        // empty. This is the only place lazy GC is observable
        // without spinning the reaper, so it doubles as the
        // regression for "GC actually fires inside the count walk".
        let hub = fresh_hub(Duration::from_millis(20)).await;
        hub.reserve_target("sess-old", "alpha").await;
        // Sanity: present immediately after reserve.
        assert_eq!(
            hub.count_live_sessions_per_target().await.get("alpha"),
            Some(&1)
        );

        tokio::time::sleep(Duration::from_millis(60)).await;

        let counts = hub.count_live_sessions_per_target().await;
        assert!(
            !counts.contains_key("alpha"),
            "expired reservation should be GCed; got {counts:?}"
        );
    }

    #[tokio::test]
    async fn release_reservation_drops_pending_only() {
        // `release_reservation` is the explicit error-path cleanup
        // for callers that minted a reservation but couldn't follow
        // through. It must drop a Pending entry but never touch
        // anything else — calling it with an unknown id is a no-op,
        // and calling it on a Pending leaves siblings alone.
        let hub = fresh_hub(Duration::from_secs(60)).await;
        hub.reserve_target("sess-a", "alpha").await;
        hub.reserve_target("sess-b", "beta").await;

        hub.release_reservation("sess-a").await;
        let counts = hub.count_live_sessions_per_target().await;
        assert!(
            !counts.contains_key("alpha"),
            "alpha released, got {counts:?}"
        );
        assert_eq!(counts.get("beta"), Some(&1), "beta untouched");

        // Unknown id: must not panic, must not change state.
        hub.release_reservation("does-not-exist").await;
        let counts = hub.count_live_sessions_per_target().await;
        assert_eq!(counts.get("beta"), Some(&1));
    }

    #[tokio::test]
    async fn reserve_is_idempotent_for_same_session_id() {
        // Two `reserve_target` calls for the same session id (e.g. a
        // retry after a flaky `POST /api/sessions`) must collapse to
        // a single Pending entry. The `or_insert_with` guard prevents
        // a duplicate from refreshing `reserved_at` and indefinitely
        // postponing GC, which would defeat the TTL.
        let hub = fresh_hub(Duration::from_millis(40)).await;
        hub.reserve_target("sess-a", "alpha").await;
        // Wait past half the TTL, then re-reserve. If
        // `reserved_at` got refreshed, the entry would survive the
        // second sleep below; the assertion catches that.
        tokio::time::sleep(Duration::from_millis(30)).await;
        hub.reserve_target("sess-a", "alpha").await;
        tokio::time::sleep(Duration::from_millis(30)).await;

        let counts = hub.count_live_sessions_per_target().await;
        assert!(
            !counts.contains_key("alpha"),
            "second reserve must NOT refresh the TTL; got {counts:?}"
        );
    }
}
