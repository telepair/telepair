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
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "Linux PTY master silently accepts writes after the child exits \
              (kernel buffers them); the writer thread never observes EPIPE, \
              so this invariant only holds on macOS where the master closes \
              more eagerly."
)]
async fn write_after_child_exit_eventually_fails() {
    let mut pty = PtyManager::spawn_command("true", &[], 80, 24, &HashMap::new()).unwrap();
    while pty.read().await.is_some() {}
    // The writer thread detects the broken pipe lazily — keep pushing
    // data until the channel closes or we give up after a generous
    // window. 4 KiB per iteration saturates any reasonable kernel
    // buffer quickly.
    let payload = Bytes::from(vec![b'x'; 4096]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut failed = false;
    while tokio::time::Instant::now() < deadline {
        if pty.write(payload.clone()).await.is_err() {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
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

/// Regression for P0-3: `impl Drop for PtyManager` used to call
/// `child.wait()` inline, blocking whatever thread the drop ran on.
/// Under Tokio that was a worker thread, and a burst of session
/// teardowns could starve the async pool. The fix offloads `wait()`
/// to the blocking pool when a Tokio runtime is available, falling
/// back to synchronous wait only when there is no runtime in scope.
///
/// This test verifies two invariants the fix guarantees:
///
/// 1. Dropping a `PtyManager` inside a Tokio runtime does not panic
///    — a regression that replaced `try_current()` with an
///    unconditional `spawn_blocking` would still work here, but a
///    broken offload that touched a dropped runtime handle would
///    surface as a panic on the test thread.
/// 2. Drop returns within a generous budget even when the child is
///    long-running. Pre-fix this was bounded by `waitpid()` latency
///    (typically sub-100 ms on Unix but platform-dependent); the
///    fix makes it unconditionally sub-millisecond in the hot path.
///
/// The time budget is intentionally loose (500 ms) so the test is
/// not flaky on slow CI; its purpose is to catch "drop hangs" not
/// to microbenchmark.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_does_not_stall_tokio_worker() {
    // `sleep 30` would outlive the test without an explicit kill,
    // so the drop path is the only thing that reaps it.
    let pty =
        PtyManager::spawn_command("sleep", &["30"], 80, 24, &HashMap::new()).unwrap();

    let start = std::time::Instant::now();
    drop(pty);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(500),
        "Drop(PtyManager) took {elapsed:?}; expected it to return \
         promptly by offloading child.wait() to spawn_blocking. \
         A large delay suggests the Drop impl regressed to inline \
         blocking wait, which would stall Tokio workers under load."
    );
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
