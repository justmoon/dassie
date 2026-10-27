//! `dassie setup` against a standalone daemon: it claims a fresh node from a
//! passphrase file, changes nothing when run again, refuses another
//! passphrase without touching the node, and adds a settlement scheme
//! (spec/areas/node.md#the-dassie-command).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use dassie_identity::{Created, Phrase, PhraseSecret, Realm};

const DASSIE: &str = env!("CARGO_BIN_EXE_dassie");
/// Ports no other test of this crate uses, since the tests of a crate run
/// at once.
const PORTS: [&str; 2] = ["18310", "18311"];

/// How long a daemon may take to start before it is called a hang: a
/// process starting is real time by nature, and a loaded machine starts it
/// several times slower, so this is far past any start, a hang's guard
/// rather than what decides the test (spec/areas/testing.md#levels).
const HANG: Duration = Duration::from_secs(60);

fn describe(output: &Output) -> String {
    format!(
        "{}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A standalone daemon on `state`, alone: no bootstrap nodes and no relay,
/// so it dials nothing outside, with its socket at `ipc`, once it answers.
fn start(state: &Path, ipc: &Path) -> Child {
    let mut child = Command::new(DASSIE)
        .args(["daemon", "--http-port", PORTS[0], "--https-port", PORTS[1]])
        .args(["--listen", "127.0.0.1", "--feed-listen", "127.0.0.1:0"])
        .env_clear()
        .env("DASSIE_STATE_DIRECTORY", state)
        .env("DASSIE_IPC_SOCKET_PATH", ipc)
        .env("DASSIE_BOOTSTRAP_NODES", "[]")
        .env("DASSIE_RELAY", "")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + HANG;
    while !ipc.exists() {
        let exited = child.try_wait().unwrap().is_some();
        if exited || Instant::now() > deadline {
            let _ = child.kill();
            panic!(
                "the daemon did not answer:\n{}",
                describe(&child.wait_with_output().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

fn stop(child: Child) -> Output {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    child.wait_with_output().unwrap()
}

fn dassie(args: &[&str], ipc: &Path) -> Output {
    Command::new(DASSIE)
        .args(args)
        .env_clear()
        .env("DASSIE_IPC_SOCKET_PATH", ipc)
        .output()
        .unwrap()
}

/// A passphrase file of a test-realm phrase whose secret is `byte`
/// sixteen times, and its node ID.
fn phrase_file(dir: &Path, name: &str, byte: u8) -> (String, String) {
    let created = Created::from_hours(7_000).unwrap();
    let phrase = Phrase::new(Realm::Test, created, &PhraseSecret::from_bytes([byte; 16]));
    let path = dir.join(name);
    std::fs::write(&path, format!("{}\n", phrase.text().as_str())).unwrap();
    let key = phrase.master_seed(Realm::Test).unwrap().node_key();
    let id = key.node_id().as_str().to_owned();
    (path.to_str().unwrap().to_owned(), id)
}

#[test]
fn setup_claims_a_node_once_and_refuses_another_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let (state, ipc) = (dir.path().join("state"), dir.path().join("ipc.sock"));
    let (owner, id) = phrase_file(dir.path(), "owner", 7);
    let (stranger, other_id) = phrase_file(dir.path(), "stranger", 9);
    let child = start(&state, &ipc);

    let first = dassie(&["setup", "--passphrase-file", &owner], &ipc);
    let text = describe(&first);
    assert!(first.status.success(), "{text}");
    assert!(
        text.contains(&format!("Set the node up as {id}, in the test realm.")),
        "{text}"
    );
    assert!(text.contains("The node's engine is running."), "{text}");

    let again = dassie(&["setup", "--passphrase-file", &owner], &ipc);
    let text = describe(&again);
    assert!(again.status.success(), "{text}");
    assert!(
        text.contains(&format!("The node is set up already as {id}.")),
        "{text}"
    );

    // A phrase names its realm, and is refused as another's.
    let live = dassie(
        &["setup", "--passphrase-file", &owner, "--realm", "live"],
        &ipc,
    );
    let text = describe(&live);
    assert_eq!(live.status.code(), Some(1), "{text}");
    assert!(text.contains("phrase of the test realm"), "{text}");

    let refused = dassie(
        &["setup", "--passphrase-file", &stranger, "--scheme", "stub"],
        &ipc,
    );
    let text = describe(&refused);
    assert_eq!(refused.status.code(), Some(3), "{text}");
    assert!(
        text.contains(&format!("set up as {id}, not as {other_id}")),
        "{text}"
    );
    // Not even the scheme it was given.
    let status = describe(&dassie(&["status"], &ipc));
    assert!(status.contains(&format!("the node ID is {id}")), "{status}");
    assert!(!status.contains("stub+"), "{status}");

    let scheme = dassie(
        &["setup", "--passphrase-file", &owner, "--scheme", "stub"],
        &ipc,
    );
    let text = describe(&scheme);
    assert!(scheme.status.success(), "{text}");
    assert!(text.contains("Added the settlement scheme stub."), "{text}");
    let status = describe(&dassie(&["status"], &ipc));
    assert!(status.contains("stub+"), "{status}");

    sets_an_accounts_config(&owner, &ipc);

    let unchanged = dassie(
        &["setup", "--passphrase-file", &owner, "--scheme", "stub"],
        &ipc,
    );
    let text = describe(&unchanged);
    assert!(unchanged.status.success(), "{text}");
    assert!(
        text.contains("The settlement scheme stub is active already."),
        "{text}"
    );

    let log = describe(&stop(child));
    // The phrase never reaches the node, so its log cannot hold it.
    let phrase = std::fs::read_to_string(&owner).unwrap();
    let first_words: String = phrase.split(' ').take(3).collect::<Vec<_>>().join(" ");
    assert!(!log.contains(&first_words), "{log}");
}

/// `dassie setup --asset` sets an account's config apart from its schemes,
/// checked as the account reads it, and the same again changes nothing.
fn sets_an_accounts_config(owner: &str, ipc: &Path) {
    let account = r#"xrpl-testnet-account/0={"server":"wss://ledger.example","faucet":null}"#;
    for _ in 0..2 {
        let asset = dassie(
            &["setup", "--passphrase-file", owner, "--asset", account],
            ipc,
        );
        let text = describe(&asset);
        assert!(asset.status.success(), "{text}");
        assert!(
            text.contains("Set the config of xrpl-testnet-account/0."),
            "{text}"
        );
    }
    let bad = r#"xrpl-testnet-account/0={"server":"https://ledger.example"}"#;
    let refused = dassie(&["setup", "--passphrase-file", owner, "--asset", bad], ipc);
    let text = describe(&refused);
    assert_eq!(refused.status.code(), Some(1), "{text}");
    assert!(
        text.contains("Could not set the config of xrpl-testnet-account/0"),
        "{text}"
    );
}

#[test]
fn setup_explains_a_missing_daemon_and_a_bad_file() {
    let dir = tempfile::tempdir().unwrap();
    let ipc = dir.path().join("absent.sock");
    let (owner, _) = phrase_file(dir.path(), "owner", 7);
    let missing = dassie(&["setup", "--passphrase-file", &owner], &ipc);
    assert_eq!(missing.status.code(), Some(1));
    assert!(describe(&missing).contains("daemon is not running"));

    let bad = dir.path().join("bad");
    std::fs::write(&bad, "nothing like a passphrase").unwrap();
    let refused = dassie(&["setup", "--passphrase-file", bad.to_str().unwrap()], &ipc);
    let text = describe(&refused);
    assert_eq!(refused.status.code(), Some(1), "{text}");
    assert!(text.contains("holds no valid passphrase"), "{text}");
    assert!(!text.contains("nothing like"), "{text}");

    let usage = dassie(&["setup"], &ipc);
    assert_eq!(usage.status.code(), Some(2));
    assert!(describe(&usage).contains("--passphrase-file is required"));
}
