use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::Read;
use tokio::sync::mpsc;
use tokio::task;

pub struct PtyManager {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    input_tx: mpsc::Sender<Vec<u8>>,
}

impl PtyManager {
    pub fn spawn_shell(cols: u16, rows: u16) -> std::io::Result<Self> {
        let shell = crate::default_shell();
        Self::spawn_command(&shell, &[], cols, rows, &HashMap::new())
    }

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
        for (key, value) in env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(std::io::Error::other)?;

        // Drop the slave side — we only use master
        drop(pair.slave);

        let mut writer = pair
            .master
            .take_writer()
            .map_err(std::io::Error::other)?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(std::io::Error::other)?;

        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(256);

        // Spawn blocking reader thread
        task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Spawn blocking writer thread
        let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(256);
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

    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.output_rx.recv().await
    }

    pub async fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.input_tx
            .send(data.to_vec())
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

    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn is_alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
