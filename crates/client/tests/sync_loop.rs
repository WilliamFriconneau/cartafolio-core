#![cfg(all(feature = "sync", unix))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A sync loop a node starts (`memcp-sync --every`, with `MEMCP_SYNC_PARENT`) ends when that node is gone: killed,
//! the node must not leave a loop behind it that a restarted node would double.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn a_loop_started_by_a_node_ends_when_the_node_is_gone() {
    let dir = std::env::temp_dir().join(format!("memcp-sync-loop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    pensine_core::Brain::init(&dir, &[], pensine_core::text::Language::English).unwrap();
    // A stand-in for the node: a shell that starts the loop as the node does, says its PID, and dies at once.
    let out = Command::new("sh")
        .arg("-c")
        .arg(r#"MEMCP_SYNC_PARENT=$$ "$0" --data "$1" --peer http://127.0.0.1:9/mcp/peer-x --name peer-x --every 5 >/dev/null 2>&1 & echo $!"#)
        .arg(env!("CARGO_BIN_EXE_memcp-sync"))
        .arg(&dir)
        .env("MEMCP_PEER_TOKEN", "hcp_test_0000")
        .output()
        .unwrap();
    let pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!pid.is_empty(), "no loop started");
    let end = Instant::now() + Duration::from_secs(20);
    while alive(&pid) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    let left = alive(&pid);
    if left {
        // The process this test started: it goes with the test.
        let _ = Command::new("kill").args(["-KILL", &pid]).status();
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!left, "the loop outlived the node that started it");
}
