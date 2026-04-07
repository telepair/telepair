use bytes::{Bytes, BytesMut};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::Read;
use tokio::sync::mpsc;
use tokio::task;

pub struct PtyManager {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    output_rx: mpsc::Receiver<Bytes>,
    input_tx: mpsc::Sender<Bytes>,
}

impl PtyManager {
    pub fn spawn_command(
        command: &str,
        args: &[&str],
        cols: u16,
        rows: u16,
        env: &HashMap<String, String>,
    ) -> std::io::Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(*arg);
        }
        // Clear inherited environment to prevent leaking server secrets to PTY sessions
        cmd.env_clear();
        // Restore minimum safe environment variables
        let safe_vars = ["HOME", "PATH", "USER", "SHELL", "LANG", "LC_ALL"];
        for var in &safe_vars {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        cmd.env("TERM", "xterm-256color");
        // Apply explicit env overrides from target config
        for (key, value) in env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(std::io::Error::other)?;

        // Drop the slave side — we only use master
        drop(pair.slave);

        let mut writer = pair.master.take_writer().map_err(std::io::Error::other)?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(std::io::Error::other)?;

        let (output_tx, output_rx) = mpsc::channel::<Bytes>(256);

        // Spawn blocking reader thread. Each chunk is split off a reusable
        // BytesMut — callers downstream clone the resulting `Bytes` by bumping
        // its Arc, avoiding the per-chunk `Vec<u8>` allocation that preceded
        // this refactor.
        task::spawn_blocking(move || {
            const CHUNK: usize = 4096;
            let mut buf = BytesMut::with_capacity(CHUNK);
            loop {
                if buf.capacity() - buf.len() < CHUNK {
                    buf.reserve(CHUNK);
                }
                // SAFETY-equivalent: BytesMut exposes spare_capacity_mut only
                // on nightly, so we write via a stack scratch buffer and
                // extend_from_slice. The extra copy is 4 KB max and stays in
                // L1; the win is removing the heap allocation per chunk.
                let mut scratch = [0u8; CHUNK];
                match reader.read(&mut scratch) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&scratch[..n]);
                        let chunk = buf.split().freeze();
                        if output_tx.blocking_send(chunk).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Spawn blocking writer thread
        let (input_tx, mut input_rx) = mpsc::channel::<Bytes>(256);
        task::spawn_blocking(move || {
            use std::io::Write;
            while let Some(data) = input_rx.blocking_recv() {
                if writer.write_all(&data).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            master: pair.master,
            child,
            output_rx,
            input_tx,
        })
    }

    pub async fn read(&mut self) -> Option<Bytes> {
        self.output_rx.recv().await
    }

    pub async fn write(&mut self, data: Bytes) -> std::io::Result<()> {
        self.input_tx
            .send(data)
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "PTY writer closed"))
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        // Reap the child when the manager is dropped. Keeping this in
        // Drop (and only in Drop) means callers can't "shutdown then
        // keep using" a half-dead manager.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
