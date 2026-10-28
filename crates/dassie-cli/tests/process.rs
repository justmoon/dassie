//! The `dassie` binary as a process: readiness and shutdown under systemd's
//! socket activation, the checks it makes before starting, and the
//! standalone runtime's socket file.
//!
//! The socket-activation tests need `systemd-socket-activate` (in the Nix
//! shell and in CI); without it they say so and pass.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The binary, as cargo and nextest name it to a test they run, never
/// `env!`'s from when the test compiled: a build is reused in other
/// checkouts of the repository.
#[allow(clippy::disallowed_methods, clippy::expect_used)] // What the test runner says, not configuration.
fn binary() -> String {
    std::env::var("CARGO_BIN_EXE_dassie").expect("cargo and nextest name the binary")
}

/// How long a daemon may take to start or stop before it is called a hang:
/// a process starting is real time by nature, and a loaded machine starts
/// it several times slower, so this is far past any start, a hang's guard
/// rather than what decides the test (spec/areas/testing.md#levels).
const HANG: Duration = Duration::from_secs(60);

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

/// The next datagram on `socket`, as text, waiting at most [`HANG`]. If
/// none comes, fails with what `child` printed.
fn receive(socket: &UnixDatagram, child: &mut Option<Child>) -> String {
    socket.set_read_timeout(Some(HANG)).unwrap();
    let mut buffer = [0; 256];
    match socket.recv(&mut buffer) {
        Ok(length) => String::from_utf8_lossy(&buffer[..length]).into_owned(),
        Err(error) => panic!(
            "no notification within {HANG:?} ({error}):\n{}",
            fail(child)
        ),
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

/// Waits for `path` to appear, until a hang ([`HANG`]); if it does not, or
/// `child` ends first, fails with what `child` printed.
fn wait_for(path: &Path, child: &mut Option<Child>) {
    let deadline = Instant::now() + HANG;
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

// Under systemd the daemon reports readiness itself once it serves, and
// says when it is stopping.
#[test]
fn socket_activated_daemon_reports_ready_and_stopping() {
    if !have_socket_activate() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let notify_path = dir.path().join("notify");
    let notify = UnixDatagram::bind(&notify_path).unwrap();
    let ipc = dir.path().join("ipc.sock");
    // One activator listens on datagrams only, so the QUIC socket comes from
    // an outer one, on the HTTPS port's number as under systemd, which the
    // inner passes on first.
    let child = Command::new("systemd-socket-activate")
        .args([
            "--now",
            "--datagram",
            "-l",
            "127.0.0.1:18295",
            "systemd-socket-activate",
            "--now",
            "-l",
            "127.0.0.1:18294",
            "-l",
            "127.0.0.1:18295",
            "-l",
        ])
        .arg(&ipc)
        .arg("--fdname=dassie-quic.socket:dassie-http.socket:dassie-https.socket:dassie-ipc.socket")
        .arg(format!(
            "--setenv=DASSIE_STATE_DIRECTORY={}",
            dir.path().join("state").display()
        ))
        .arg(format!("--setenv=NOTIFY_SOCKET={}", notify_path.display()))
        .args([
            "--setenv=DASSIE_BOOTSTRAP_NODES=[]",
            "--setenv=DASSIE_RELAY=",
        ])
        .args([binary().as_str(), "daemon"])
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
    // Without the optional rate feed's socket, the node runs, and says so;
    // it takes QUIC on the socket it was given.
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(log.contains("dassie-feed.socket"), "{log}");
    assert!(log.contains("feed=none"), "{log}");
    assert!(log.contains("quic=127.0.0.1:18295"), "{log}");
}

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(binary())
        .args(args)
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .unwrap()
}

// Sockets whose LISTEN_PID names another process are not taken.
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

// A bad variable stops the daemon before anything starts, naming it.
#[test]
fn a_bad_variable_stops_the_daemon_with_its_name() {
    let output = run(
        &["daemon"],
        &[("HOME", "/nonexistent"), ("DASSIE_LOG", "verbose")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("DASSIE_LOG"));
}

// The standalone runtime makes its socket 0660, and removes it when it
// stops.
#[test]
fn the_standalone_daemon_cleans_up_its_socket() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ipc = dir.path().join("ipc.sock");
    let state = dir.path().join("state");
    let child = Command::new(binary())
        .args(["daemon", "--http-port", "18290", "--https-port", "18291"])
        .env_clear()
        .env("DASSIE_STATE_DIRECTORY", &state)
        .env("DASSIE_IPC_SOCKET_PATH", &ipc)
        .env("DASSIE_BOOTSTRAP_NODES", "[]")
        .env("DASSIE_RELAY", "")
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

// The dev environment keeps its nodes on loopback: `--listen` binds only
// there.
#[test]
fn the_standalone_daemon_listens_where_it_is_told() {
    use std::net::{SocketAddr, TcpStream};
    let dir = tempfile::tempdir().unwrap();
    let ipc = dir.path().join("ipc.sock");
    let child = Command::new(binary())
        .args(["daemon", "--http-port", "18296", "--https-port", "18297"])
        .args(["--listen", "127.0.0.1"])
        .env_clear()
        .env("DASSIE_STATE_DIRECTORY", dir.path().join("state"))
        .env("DASSIE_IPC_SOCKET_PATH", &ipc)
        .env("DASSIE_BOOTSTRAP_NODES", "[]")
        .env("DASSIE_RELAY", "")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut child = Some(child);
    wait_for(&ipc, &mut child);
    assert!(TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], 18297))).is_ok());
    // Not on IPv6 loopback, as it would be when bound to every address.
    let v6: SocketAddr = "[::1]:18297".parse().unwrap();
    assert!(TcpStream::connect(v6).is_err());
    terminate(pid);
    let output = child.take().unwrap().wait_with_output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
}

// The CLI explains a daemon that is not running, rather than failing with
// a stack trace.
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

/// Starts a standalone daemon on `state`, alone (no bootstrap nodes and no
/// relay, so it dials nothing outside), with its socket at `ipc`, and
/// waits until it answers.
fn start_standalone(state: &Path, ipc: &Path, ports: [&str; 2]) -> Option<Child> {
    let child = Command::new(binary())
        .args(["daemon", "--http-port", ports[0], "--https-port", ports[1]])
        .args(["--listen", "127.0.0.1"])
        .env_clear()
        .env("DASSIE_STATE_DIRECTORY", state)
        .env("DASSIE_IPC_SOCKET_PATH", ipc)
        .env("DASSIE_BOOTSTRAP_NODES", "[]")
        .env("DASSIE_RELAY", "")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = Some(child);
    wait_for(ipc, &mut child);
    child
}

fn stop(child: &mut Option<Child>) -> Output {
    let child = child.take().unwrap();
    terminate(child.id());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    output
}

// The original drew a new setup token at every start, and had no status
// command: an owner back the day after an update found a dead link.
#[test]
fn status_and_the_setup_url_before_setup() {
    let dir = tempfile::tempdir().unwrap();
    let (state, ipc) = (dir.path().join("state"), dir.path().join("ipc.sock"));
    let env = [("DASSIE_IPC_SOCKET_PATH", ipc.to_str().unwrap())];
    let mut child = start_standalone(&state, &ipc, ["18298", "18299"]);
    let url = run(&["setup-url"], &env);
    assert!(url.status.success(), "{}", describe(&url));
    let url = String::from_utf8(url.stdout).unwrap();
    let token = url.trim().rsplit('/').next().unwrap();
    assert_eq!(token.len(), 64, "{url}");
    let status = run(&["status"], &env);
    assert!(status.status.success(), "{}", describe(&status));
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains(" is running, for "), "{text}");
    assert!(text.contains("Setup        not done yet"), "{text}");
    // Without a hostname it runs as a periphery node, which needs no domain
    // name: status does not send its owner to `dassie init`.
    assert!(
        text.contains("Address      none yet: a periphery node is named once it is set up"),
        "{text}"
    );
    assert!(!text.contains("dassie init"), "{text}");
    // It names no secret, so it can be pasted where help is asked for.
    assert!(!text.contains(token), "{text}");
    // Nor does the daemon's log, which the journal keeps for groups that do
    // not administer the node: it says where to get the URL instead.
    let log = describe(&stop(&mut child));
    assert!(!log.contains(token), "{log}");
    assert!(log.contains("dassie setup-url"), "{log}");
    let stopped = run(&["status"], &env);
    assert_eq!(stopped.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("Dassie is not running."));
    let mut child = start_standalone(&state, &ipc, ["18298", "18299"]);
    let again = run(&["setup-url"], &env);
    assert_eq!(String::from_utf8(again.stdout).unwrap(), url);
    stop(&mut child);
}
