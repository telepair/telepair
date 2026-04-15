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
    // Drop runs the child reap — no explicit shutdown API.
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
    pty.resize(120, 40).unwrap();
}

#[test]
fn spawn_nonexistent_binary_returns_error() {
    let result =
        PtyManager::spawn_command("/nonexistent/binary/path", &[], 80, 24, &HashMap::new());
    assert!(result.is_err(), "spawning a missing binary must fail");
}

#[tokio::test]
async fn write_after_child_exit_eventually_fails() {
    let mut pty = PtyManager::spawn_command("true", &[], 80, 24, &HashMap::new()).unwrap();
    while pty.read().await.is_some() {}
    // The writer thread detects the broken pipe lazily — keep pushing
    // data until the channel closes or we give up after a generous window.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let mut failed = false;
    while tokio::time::Instant::now() < deadline {
        if pty.write(Bytes::from_static(b"data\n")).await.is_err() {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(failed, "writing to a dead PTY must eventually fail");
}

#[tokio::test]
async fn env_vars_are_passed_to_child() {
    let mut env = HashMap::new();
    env.insert("TELEPAIR_TEST_MARKER".into(), "present".into());
    let mut pty = PtyManager::spawn_command("env", &[], 80, 24, &env).unwrap();
    let output = timeout(Duration::from_secs(3), async {
        let mut all = Vec::new();
        loop {
            match pty.read().await {
                Some(data) => {
                    all.extend_from_slice(&data);
                    let text = String::from_utf8_lossy(&all);
                    if text.contains("TELEPAIR_TEST_MARKER=present") {
                        return text.to_string();
                    }
                }
                None => return String::from_utf8_lossy(&all).to_string(),
            }
        }
    })
    .await
    .expect("timed out");
    assert!(output.contains("TELEPAIR_TEST_MARKER=present"));
}

#[tokio::test]
async fn server_secrets_not_leaked_to_child() {
    unsafe { std::env::set_var("TELEPAIR_SECRET_PROBE", "leaked") };
    let mut pty = PtyManager::spawn_command("env", &[], 80, 24, &HashMap::new()).unwrap();
    let output = timeout(Duration::from_secs(3), async {
        let mut all = Vec::new();
        loop {
            match pty.read().await {
                Some(data) => all.extend_from_slice(&data),
                None => return String::from_utf8_lossy(&all).to_string(),
            }
        }
    })
    .await
    .expect("timed out");
    unsafe { std::env::remove_var("TELEPAIR_SECRET_PROBE") };
    assert!(
        !output.contains("TELEPAIR_SECRET_PROBE"),
        "env_clear must prevent server env from leaking into PTY"
    );
}
