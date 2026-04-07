use bytes::Bytes;
use std::collections::HashMap;
use telepair_agent::pty::PtyManager;
use tokio::time::{Duration, timeout};

fn spawn_test_shell(cols: u16, rows: u16) -> PtyManager {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    PtyManager::spawn_command(&shell, &[], cols, rows, &HashMap::new()).unwrap()
}

#[tokio::test]
async fn spawn_shell_and_read_output() {
    let mut pty = spawn_test_shell(80, 24);

    // Write a command
    pty.write(Bytes::from_static(b"echo HELLO_TELEPAIR\n"))
        .await
        .unwrap();

    // Read output until we see our marker
    let output = timeout(Duration::from_secs(5), async {
        let mut all_output = Vec::new();
        loop {
            if let Some(data) = pty.read().await {
                all_output.extend_from_slice(&data);
                let text = String::from_utf8_lossy(&all_output);
                if text.contains("HELLO_TELEPAIR") {
                    return text.to_string();
                }
            }
        }
    })
    .await
    .expect("timed out waiting for output");

    assert!(output.contains("HELLO_TELEPAIR"));
    pty.shutdown();
}

#[tokio::test]
async fn spawn_command() {
    let mut pty =
        PtyManager::spawn_command("echo", &["PTY_TEST"], 80, 24, &HashMap::new()).unwrap();

    let output = timeout(Duration::from_secs(3), async {
        let mut all = Vec::new();
        loop {
            match pty.read().await {
                Some(data) => {
                    all.extend_from_slice(&data);
                    let text = String::from_utf8_lossy(&all);
                    if text.contains("PTY_TEST") {
                        return text.to_string();
                    }
                }
                None => return String::from_utf8_lossy(&all).to_string(),
            }
        }
    })
    .await
    .expect("timed out");

    assert!(output.contains("PTY_TEST"));
}

#[tokio::test]
async fn resize_pty() {
    let mut pty = spawn_test_shell(80, 24);
    // Should not panic
    pty.resize(120, 40).unwrap();
    pty.shutdown();
}
