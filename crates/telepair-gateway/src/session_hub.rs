use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use telepair_agent::pty::PtyManager;
use telepair_core::permission::Role;
use telepair_core::protocol::{ParticipantInfo, ServerMessage};
use telepair_core::storage::{SqliteStorage, Storage};

/// Color palette for participant cursors / identifiers.
const COLORS: &[&str] = &[
    "#58a6ff", "#3fb950", "#d29922", "#f85149", "#bc8cff", "#39c5cf", "#ffa198", "#56d364",
];

fn assign_color(index: usize) -> String {
    COLORS[index % COLORS.len()].to_string()
}

/// Commands sent to the PTY I/O loop.
pub enum PtyCommand {
    Input(Bytes),
    Resize(u16, u16),
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
    ) -> Result<
        (
            mpsc::Sender<PtyCommand>,
            broadcast::Receiver<Bytes>,
            broadcast::Receiver<ServerMessage>,
            broadcast::Receiver<()>,
        ),
        String,
    > {
        // Fast path: session already running — subscribe under read lock
        {
            let sessions = self.sessions.read().await;
            if let Some(live) = sessions.get(session_id) {
                return Ok((
                    live.cmd_tx.clone(),
                    live.output_tx.subscribe(),
                    live.collab_tx.subscribe(),
                    live.shutdown_tx.subscribe(),
                ));
            }
        }

        // Spawn PTY without holding any lock (fork/exec can be slow)
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut pty = PtyManager::spawn_command(command, &args_ref, cols, rows, env)
            .map_err(|e| e.to_string())?;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(256);
        let (output_tx, output_rx) = broadcast::channel::<Bytes>(256);
        let (collab_tx, collab_rx) = broadcast::channel::<ServerMessage>(64);
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

        // Re-acquire write lock and check for TOCTOU race
        let mut sessions = self.sessions.write().await;
        if let Some(live) = sessions.get(session_id) {
            // Another task spawned the session while we were spawning — use theirs,
            // our PTY will be dropped and the child process cleaned up
            drop(pty);
            return Ok((
                live.cmd_tx.clone(),
                live.output_tx.subscribe(),
                live.collab_tx.subscribe(),
                live.shutdown_tx.subscribe(),
            ));
        }

        let output_tx_clone = output_tx.clone();
        let session_id_owned = session_id.to_string();
        let sessions_arc = self.sessions.clone();
        let storage_clone = self.storage.clone();

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
                        // Already a refcounted Bytes from the PTY reader;
                        // every subscriber clone is an Arc bump, not a copy.
                        let _ = output_tx_clone.send(bytes);
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

            // Cleanup: remove from in-memory map and close in DB
            sessions_arc.write().await.remove(&session_id_owned);
            if let Err(e) = storage_clone.close_session(&session_id_owned).await {
                tracing::warn!(session = %session_id_owned, "failed to close session in DB: {e}");
            }
        });

        let live = LiveSession {
            cmd_tx: cmd_tx.clone(),
            output_tx: output_tx.clone(),
            collab_tx: collab_tx.clone(),
            shutdown_tx,
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

        Ok((cmd_tx, output_rx, collab_rx, shutdown_rx))
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

    /// Register a WS connection for a participant. The first connection for
    /// a given `user_id` broadcasts `PeerJoined` and assigns a color;
    /// subsequent connections (e.g. a second tab) just bump the refcount and
    /// reuse the existing participant record. Returns `true` when the
    /// session exists and the connection was registered.
    pub async fn add_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        name: String,
        role: Role,
    ) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(live) = sessions.get_mut(session_id) else {
            return false;
        };

        // Any successful attach clears the idle clock. Do this before the
        // refcount bump so a reconnect during the reaper's grace period
        // always wins against a pending sweep.
        live.idle_since = None;

        let count = live.connections.entry(user_id).or_insert(0);
        *count += 1;
        if *count > 1 {
            // Additional connection from an already-joined user. Don't
            // re-broadcast PeerJoined or reassign a color — the existing
            // participant record stays authoritative.
            return true;
        }

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

        true
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

    /// Get a snapshot of all participants in a session.
    pub async fn get_participants(&self, session_id: &str) -> Vec<ParticipantInfo> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|live| live.participants.values().cloned().collect())
            .unwrap_or_default()
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
                            (idle_since.elapsed() >= config.idle_timeout)
                                .then(|| id.clone())
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
