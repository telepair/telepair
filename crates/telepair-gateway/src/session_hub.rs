use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{RwLock, broadcast, mpsc};
use uuid::Uuid;

use telepair_agent::pty::PtyManager;
use telepair_core::permission::Role;
use telepair_core::protocol::ServerMessage;
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
    Input(Vec<u8>),
    Resize(u16, u16),
}

/// A participant currently connected to a session.
#[derive(Debug, Clone)]
pub struct ConnectedParticipant {
    pub user_id: Uuid,
    pub name: String,
    pub role: Role,
    pub color: String,
}

/// A running terminal session with PTY, broadcast channels, and participant tracking.
struct LiveSession {
    /// Send terminal input (or resize) to PTY via command channel
    cmd_tx: mpsc::Sender<PtyCommand>,
    /// Subscribe to PTY output. Uses `Bytes` so broadcast cloning is a cheap
    /// refcount bump per subscriber instead of a per-chunk `Vec<u8>` copy.
    output_tx: broadcast::Sender<Bytes>,
    /// Broadcast collaboration messages (PeerJoined, PeerLeft, PermUpdate, etc.)
    collab_tx: broadcast::Sender<ServerMessage>,
    /// Signal to all connected WS handlers that this session is being force-stopped
    shutdown_tx: broadcast::Sender<()>,
    /// Currently connected participants, keyed by user_id. A single user may
    /// open multiple tabs/devices; the map stores one canonical record and
    /// `connections` below counts how many live WS handlers are attached.
    participants: HashMap<Uuid, ConnectedParticipant>,
    /// WS-handler reference count per user_id. Invariant: a `user_id` is in
    /// `participants` iff `connections[user_id] > 0`. Without this counter a
    /// user's second tab closing would broadcast `PeerLeft` and wipe the
    /// in-memory entry even though another tab is still connected.
    connections: HashMap<Uuid, usize>,
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
                    Output(Option<Vec<u8>>),
                    Command(Option<PtyCommand>),
                }

                let action = tokio::select! {
                    data = pty.read() => Action::Output(data),
                    cmd = cmd_rx.recv() => Action::Command(cmd),
                };

                match action {
                    Action::Output(Some(bytes)) => {
                        // Wrap once into refcounted Bytes; every subscriber
                        // clone is an Arc bump, not a byte copy.
                        let _ = output_tx_clone.send(Bytes::from(bytes));
                    }
                    Action::Output(None) => {
                        tracing::info!(session = %session_id_owned, "PTY process exited");
                        break;
                    }
                    Action::Command(Some(PtyCommand::Input(data))) => {
                        if pty.write(&data).await.is_err() {
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
            ConnectedParticipant {
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
        true
    }

    /// Get a snapshot of all participants in a session.
    pub async fn get_participants(&self, session_id: &str) -> Vec<ConnectedParticipant> {
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

    /// Update a participant's role and broadcast `PermUpdate` to all subscribers.
    pub async fn update_participant_role(
        &self,
        session_id: &str,
        user_id: Uuid,
        new_role: Role,
    ) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(live) = sessions.get_mut(session_id) else {
            return false;
        };
        let Some(participant) = live.participants.get_mut(&user_id) else {
            return false;
        };

        participant.role = new_role;

        let _ = live
            .collab_tx
            .send(ServerMessage::PermUpdate { user_id, new_role });

        true
    }
}
