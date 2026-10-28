//! `dassie setup` against a standalone daemon: it claims a fresh node from a
//! passphrase file, changes nothing when run again, refuses another
//! passphrase without touching the node, adds a settlement scheme, and
//! takes the hubs to join by their schemes (`--hub`)
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

/// `dassie setup` from the passphrase in `phrase`, with `flags`, and
/// `--hub none`: the node joins no hub built into the release, whose
/// validators and XRP Ledger server are on the public testnet, which no
/// test reaches (spec/areas/testing.md).
fn setup(phrase: &str, flags: &[&str], ipc: &Path) -> Output {
    let mut args = vec!["setup", "--passphrase-file", phrase];
    args.extend_from_slice(flags);
    args.extend_from_slice(&["--hub", "none"]);
    dassie(&args, ipc)
}

/// A passphrase file of a test-realm phrase whose secret is `byte`
/// sixteen times, and its node ID.
fn phrase_file(dir: &Path, name: &str, byte: u8) -> (String, String) {
    phrase_file_in(Realm::Test, dir, name, byte)
}

/// A passphrase file of a phrase of `realm` whose secret is `byte` sixteen
/// times, made in 2026, and its node ID.
fn phrase_file_in(realm: Realm, dir: &Path, name: &str, byte: u8) -> (String, String) {
    let created = Created::from_hours(7_000).unwrap();
    let phrase = Phrase::new(realm, created, &PhraseSecret::from_bytes([byte; 16]));
    let path = dir.join(name);
    std::fs::write(&path, format!("{}\n", phrase.text().as_str())).unwrap();
    let key = phrase.master_seed(realm).unwrap().node_key();
    let id = key.identity().node_id().as_str().to_owned();
    (path.to_str().unwrap().to_owned(), id)
}

#[test]
fn setup_claims_a_node_once_and_refuses_another_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let (state, ipc) = (dir.path().join("state"), dir.path().join("ipc.sock"));
    let (owner, id) = phrase_file(dir.path(), "owner", 7);
    let (stranger, other_id) = phrase_file(dir.path(), "stranger", 9);
    let child = start(&state, &ipc);

    a_fresh_node_refuses_a_live_phrase(dir.path(), &ipc);

    let first = setup(&owner, &[], &ipc);
    let text = describe(&first);
    assert!(first.status.success(), "{text}");
    assert!(
        text.contains(&format!("is node {id}, of the test realm.")),
        "{text}"
    );
    assert!(
        text.contains(&format!("Set the node up as {id}, in the test realm.")),
        "{text}"
    );
    assert!(text.contains("The node's engine is running."), "{text}");

    let again = setup(&owner, &[], &ipc);
    let text = describe(&again);
    assert!(again.status.success(), "{text}");
    assert!(
        text.contains(&format!("The node is set up already as {id}.")),
        "{text}"
    );

    refuses_a_phrase_of_another_realm(dir.path(), &owner, &ipc);
    names_hubs_by_their_schemes(&owner, &ipc);

    let refused = setup(&stranger, &["--scheme", "stub"], &ipc);
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

    let scheme = setup(&owner, &["--scheme", "stub"], &ipc);
    let text = describe(&scheme);
    assert!(scheme.status.success(), "{text}");
    assert!(text.contains("Added the settlement scheme stub."), "{text}");
    let status = describe(&dassie(&["status"], &ipc));
    assert!(status.contains("stub+"), "{status}");
    // No setup joined a hub built into the release.
    assert!(!status.contains("hub-xrpl"), "{status}");

    sets_an_accounts_config(&owner, &ipc);

    let unchanged = setup(&owner, &["--scheme", "stub"], &ipc);
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

/// A phrase names its realm, so no flag does, and a phrase of a realm the
/// node does not run is refused before the node is touched.
fn refuses_a_phrase_of_another_realm(dir: &Path, owner: &str, ipc: &Path) {
    let flagged = dassie(
        &["setup", "--passphrase-file", owner, "--realm", "test"],
        ipc,
    );
    let text = describe(&flagged);
    assert_eq!(flagged.status.code(), Some(2), "{text}");

    let (live_owner, _) = phrase_file_in(Realm::Live, dir, "live", 7);
    let live = dassie(&["setup", "--passphrase-file", &live_owner], ipc);
    let text = describe(&live);
    assert_eq!(live.status.code(), Some(1), "{text}");
    assert!(
        text.contains("The node runs the test realm, and the passphrase"),
        "{text}"
    );
    assert!(text.contains("a phrase of the live realm"), "{text}");
}

/// `--hub` names each hub by its scheme, or `none`. A flag line that cannot
/// mean anything is a usage error, `--no-hub` among them, which is gone; a
/// hub of another realm than the phrase's is refused before the node is
/// touched; and `--hub none` joins no hub, not even the realm's default.
fn names_hubs_by_their_schemes(owner: &str, ipc: &Path) {
    let usage = |flags: &[&str], says: &str| {
        let mut args = vec!["setup", "--passphrase-file", owner];
        args.extend_from_slice(flags);
        let refused = dassie(&args, ipc);
        let text = describe(&refused);
        assert_eq!(refused.status.code(), Some(2), "{text}");
        assert!(text.contains(says), "{text}");
    };
    usage(&["--no-hub"], "pass --hub none");
    usage(&["--hub"], "--hub");
    usage(
        &["--hub", "xrpl-testnet-xrp"],
        "names no settlement hub's scheme",
    );
    usage(
        &["--hub", "none", "--hub", "hub-xrpl-testnet-xrp"],
        "--hub none joins no settlement hub",
    );

    let live = dassie(
        &[
            "setup",
            "--passphrase-file",
            owner,
            "--scheme",
            "stub",
            "--hub",
            "hub-xrpl-xrp",
        ],
        ipc,
    );
    let text = describe(&live);
    assert_eq!(live.status.code(), Some(1), "{text}");
    assert!(
        text.contains("--hub hub-xrpl-xrp names a settlement hub of the live realm"),
        "{text}"
    );
    assert!(text.contains("Nothing was changed."), "{text}");
    let status = describe(&dassie(&["status"], ipc));
    assert!(!status.contains("stub+"), "{status}");

    let none = dassie(&["setup", "--passphrase-file", owner, "--hub", "none"], ipc);
    let text = describe(&none);
    assert!(none.status.success(), "{text}");
    assert!(!text.contains("settlement hub"), "{text}");
    assert!(!text.contains("Added the settlement scheme"), "{text}");
}

/// A fresh node that may not run the live realm, as none may but by the
/// dev switch, refuses a setup with a phrase of it, and stays as it was.
fn a_fresh_node_refuses_a_live_phrase(dir: &Path, ipc: &Path) {
    let (live_owner, live_id) = phrase_file_in(Realm::Live, dir, "live", 7);
    let live = dassie(&["setup", "--passphrase-file", &live_owner], ipc);
    let text = describe(&live);
    assert_eq!(live.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!("is node {live_id}, of the live realm.")),
        "{text}"
    );
    assert!(
        text.contains("The node refused setup in the live realm, which the passphrase names"),
        "{text}"
    );
    assert!(
        text.contains("the live realm is not supported yet"),
        "{text}"
    );
}

/// `dassie setup --asset` sets an account's config apart from its schemes,
/// checked as the account reads it, and the same again changes nothing.
fn sets_an_accounts_config(owner: &str, ipc: &Path) {
    let account = r#"xrpl-testnet-account/0={"server":"wss://ledger.example","faucet":null}"#;
    for _ in 0..2 {
        let asset = setup(owner, &["--asset", account], ipc);
        let text = describe(&asset);
        assert!(asset.status.success(), "{text}");
        assert!(
            text.contains("Set the config of xrpl-testnet-account/0."),
            "{text}"
        );
    }
    let bad = r#"xrpl-testnet-account/0={"server":"https://ledger.example"}"#;
    let refused = setup(owner, &["--asset", bad], ipc);
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

    // A phrase that says it was made in 2145 is refused by today's clock,
    // with its date.
    let last = Created::from_hours(Created::MAX_HOURS).unwrap();
    let future = Phrase::new(Realm::Test, last, &PhraseSecret::from_bytes([7; 16]));
    let path = dir.path().join("future");
    std::fs::write(&path, future.text().as_str()).unwrap();
    let refused = dassie(
        &["setup", "--passphrase-file", path.to_str().unwrap()],
        &ipc,
    );
    let text = describe(&refused);
    assert_eq!(refused.status.code(), Some(1), "{text}");
    assert!(text.contains("made on 2145-08-15"), "{text}");

    let usage = dassie(&["setup"], &ipc);
    assert_eq!(usage.status.code(), Some(2));
    assert!(describe(&usage).contains("--passphrase-file is required"));
}
