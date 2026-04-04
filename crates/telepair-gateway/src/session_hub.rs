use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, RwLock};

use telepair_agent::pty::PtyManager;

/// Commands sent to the PTY I/O loop.
pub enum PtyCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
}

/// A running terminal session with PTY and broadcast channel.
struct LiveSession {
    /// Send terminal input (or resize) to PTY via command channel
    cmd_tx: mpsc::Sender<PtyCommand>,
    /// Subscribe to PTY output
    output_tx: broadcast::Sender<Vec<u8>>,
}

pub struct SessionHub {
    sessions: Arc<RwLock<HashMap<String, LiveSession>>>,
}

impl Default for SessionHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionHub {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a PTY for a session. Returns channels for I/O.
    pub async fn start_session(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        cols: u16,
        rows: u16,
    ) -> Result<
        (
            mpsc::Sender<PtyCommand>,
            broadcast::Receiver<Vec<u8>>,
        ),
        String,
    > {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut pty =
            PtyManager::spawn_command(command, &args_ref, cols, rows).map_err(|e| e.to_string())?;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(256);
        let (output_tx, output_rx) = broadcast::channel::<Vec<u8>>(256);

        let output_tx_clone = output_tx.clone();
        let session_id_owned = session_id.to_string();
        let sessions = self.sessions.clone();

        // PTY I/O loop — single task owns the PtyManager
        tokio::spawn(async move {
            loop {
                // Use select to wait for either PTY output or a command.
                // To avoid simultaneous &mut pty borrows, we capture the
                // result into local variables and handle pty mutations after
                // the select (when all futures have been dropped).
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
                        let _ = output_tx_clone.send(bytes);
                    }
                    Action::Output(None) => {
                        // PTY closed
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
                        // All senders dropped — no more clients
                        tracing::info!(session = %session_id_owned, "all clients disconnected");
                        break;
                    }
                }
            }

            // Cleanup
            sessions.write().await.remove(&session_id_owned);
        });

        let live = LiveSession {
            cmd_tx: cmd_tx.clone(),
            output_tx: output_tx.clone(),
        };
        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), live);

        Ok((cmd_tx, output_rx))
    }

    /// Join an existing live session (get command sender + output receiver).
    pub async fn join_session(
        &self,
        session_id: &str,
    ) -> Option<(mpsc::Sender<PtyCommand>, broadcast::Receiver<Vec<u8>>)> {
        let sessions = self.sessions.read().await;
        let live = sessions.get(session_id)?;
        Some((live.cmd_tx.clone(), live.output_tx.subscribe()))
    }

    pub async fn is_live(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }
}
