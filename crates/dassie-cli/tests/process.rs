//! The `dassie` binary as a process: readiness and shutdown under systemd's
//! socket activation, the checks it makes before starting, and the
//! standalone runtime's socket file.
//!
//! The socket-activation tests need `systemd-socket-activate` (in the Nix
//! shell and in CI); without it they say so and pass.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const DASSIE: &str = env!("CARGO_BIN_EXE_dassie");

fn have_socket_activate() -> bool {
    let found = Command::new("systemd-socket-activate")
        .arg("--version")
        .output()
        .is_ok();
    if !found {
        eprintln!("systemd-socket-activate is not on PATH; skipping");
    }
    found
}

/// The next datagram on `socket`, as text, waiting at most 10 s. If none
/// comes, fails with what `child` printed.
fn receive(socket: &UnixDatagram, child: &mut Option<Child>) -> String {
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0; 256];
    match socket.recv(&mut buffer) {
        Ok(length) => String::from_utf8_lossy(&buffer[..length]).into_owned(),
        Err(error) => panic!("no notification within 10 s ({error}):\n{}", fail(child)),
    }
}

/// Stops `child` and describes how it ended, for a failure message.
fn fail(child: &mut Option<Child>) -> String {
    let Some(mut child) = child.take() else {
        return String::new();
    };
    let _ = child.kill();
    describe(&child.wait_with_output().unwrap())
}

fn describe(output: &Output) -> String {
    format!(
        "{}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn terminate(pid: u32) {
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Waits for `path` to appear; if it does not, fails with what `child`
/// printed.
fn wait_for(path: &Path, child: &mut Option<Child>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        let exited = child
            .as_mut()
            .is_some_and(|c| c.try_wait().unwrap().is_some());
        if exited || Instant::now() > deadline {
            panic!("{} did not appear:\n{}", path.display(), fail(child));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// RT-20: the original ran `systemd-notify --ready`, after its first ACME
// check, and never said it was stopping.
#[test]
fn socket_activated_daemon_reports_ready_and_stopping() {
    if !have_socket_activate() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let notify_path = dir.path().join("notify");
    let notify = UnixDatagram::bind(&notify_path).unwrap();
    let ipc = dir.path().join("ipc.sock");
    let child = Command::new("systemd-socket-activate")
        .args([
            "--now",
            "-l",
            "127.0.0.1:18294",
            "-l",
            "127.0.0.1:18295",
            "-l",
        ])
        .arg(&ipc)
        .arg("--fdname=dassie-http.socket:dassie-https.socket:dassie-ipc.socket")
        .arg(format!(
            "--setenv=DASSIE_STATE_DIRECTORY={}",
            dir.path().join("state").display()
        ))
        .arg(format!("--setenv=NOTIFY_SOCKET={}", notify_path.display()))
        .args([DASSIE, "daemon"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut child = Some(child);
    assert_eq!(receive(&notify, &mut child), "READY=1");
    terminate(pid);
    assert_eq!(receive(&notify, &mut child), "STOPPING=1");
    let output = child.take().unwrap().wait_with_output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(DASSIE)
        .args(args)
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .unwrap()
}

// RT-20: the original did not compare LISTEN_PID with its own.
#[test]
fn sockets_meant_for_another_process_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let env = [
        ("DASSIE_STATE_DIRECTORY", state.to_str().unwrap()),
        ("LISTEN_PID", "1"),
        ("LISTEN_FDS", "3"),
        (
            "LISTEN_FDNAMES",
            "dassie-http.socket:dassie-https.socket:dassie-ipc.socket",
        ),
    ];
    let output = run(&["daemon", "--runtime", "systemd"], &env);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("not this process"));
}

// RT-24: the original ran on half-started when a variable was bad.
#[test]
fn a_bad_variable_stops_the_daemon_with_its_name() {
    let output = run(
        &["daemon"],
        &[("HOME", "/nonexistent"), ("DASSIE_LOG_LEVEL", "verbose")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("DASSIE_LOG_LEVEL"));
}

// RT-7 and RT-9: the standalone runtime makes its socket 0660, and removes
// it when it stops.
#[test]
fn the_standalone_daemon_cleans_up_its_socket() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ipc = dir.path().join("ipc.sock");
    let state = dir.path().join("state");
    let child = Command::new(DASSIE)
        .args(["daemon", "--http-port", "18290", "--https-port", "18291"])
        .env_clear()
        .env("DASSIE_STATE_DIRECTORY", &state)
        .env("DASSIE_IPC_SOCKET_PATH", &ipc)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut child = Some(child);
    wait_for(&ipc, &mut child);
    assert_eq!(
        std::fs::metadata(&ipc).unwrap().permissions().mode() & 0o777,
        0o660
    );
    // A second daemon on the same socket refuses to start.
    let second = run(
        &["daemon", "--http-port", "18292", "--https-port", "18293"],
        &[
            ("DASSIE_STATE_DIRECTORY", state.to_str().unwrap()),
            ("DASSIE_IPC_SOCKET_PATH", ipc.to_str().unwrap()),
        ],
    );
    assert_eq!(second.status.code(), Some(1), "{}", describe(&second));
    terminate(pid);
    let output = child.take().unwrap().wait_with_output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    assert!(!ipc.exists());
}

// RT-9: the original's client crashed with a stack trace when the daemon
// was not running.
#[test]
fn init_explains_a_missing_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let ipc = dir.path().join("absent.sock");
    let output = run(
        &["init", "--hostname", "node.example"],
        &[("DASSIE_IPC_SOCKET_PATH", ipc.to_str().unwrap())],
    );
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8_lossy(&output.stderr) + String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("daemon is not running"), "{text}");
}

#[test]
fn verify_install_checks_the_database_and_tls() {
    let output = run(&["verify-install"], &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Verifying that SQLite is working..."));
    assert!(stdout.contains("Verifying that TLS is working..."));
    // Whether it succeeds depends on whether this build has the wallet.
    assert_eq!(output.status.success(), stdout.contains("Success!"));
}
