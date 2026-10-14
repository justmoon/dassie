#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use dassie_node_core::DAY_MS;
use dassie_node_core::config::Hostname;

use super::*;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn session(byte: u8, created: u64, used: u64) -> SessionRecord {
    SessionRecord {
        id: SessionId([byte; 32]),
        times: SessionTimes {
            created_at_ms: created,
            last_used_at_ms: used,
        },
        user_agent: "test".into(),
    }
}

// RT-25: the original created the database with the default umask.
#[test]
fn creates_a_private_directory_and_file() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let store = Store::open(&data).unwrap();
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&data.join(DATABASE_FILE)), 0o600);
    assert_eq!(store.self_test().unwrap(), 2);
}

#[test]
fn tightens_loose_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    drop(Store::open(&data).unwrap());
    fs::set_permissions(&data, Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(data.join(DATABASE_FILE), Permissions::from_mode(0o644)).unwrap();
    drop(Store::open(&data).unwrap());
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&data.join(DATABASE_FILE)), 0o600);
}

#[test]
fn keeps_its_data_across_opens() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let mut config = store.config().unwrap();
    config.hostname = Some(Hostname::parse("node.example").unwrap());
    store.set_config(&config).unwrap();
    drop(store);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(store.config().unwrap(), config);
}

// D3: the original's database is refused with a way forward.
#[test]
fn refuses_a_v0_database() {
    let temp = tempfile::tempdir().unwrap();
    let connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", V0_APPLICATION_ID)
        .unwrap();
    connection
        .execute_batch("CREATE TABLE scalar (key TEXT PRIMARY KEY) STRICT")
        .unwrap();
    drop(connection);
    let error = Store::open(temp.path()).unwrap_err();
    assert!(matches!(error, StoreError::V0Database));
    assert!(error.to_string().contains("passphrase"));
}

#[test]
fn refuses_foreign_and_newer_databases() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DATABASE_FILE);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE other (x)").unwrap();
    drop(connection);
    assert!(matches!(
        Store::open(temp.path()),
        Err(StoreError::ForeignDatabase(0))
    ));
    fs::remove_file(&path).unwrap();
    drop(Store::open(temp.path()).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert!(matches!(
        Store::open(temp.path()),
        Err(StoreError::TooNew {
            found: 99,
            known: 3
        })
    ));
}

// The original did not notice columns it did not expect.
#[test]
fn notices_a_changed_table() {
    let temp = tempfile::tempdir().unwrap();
    drop(Store::open(temp.path()).unwrap());
    let connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .execute_batch("ALTER TABLE sessions ADD COLUMN extra TEXT")
        .unwrap();
    drop(connection);
    let error = Store::open(temp.path()).unwrap_err();
    assert!(
        matches!(error, StoreError::SchemaMismatch(ref problem) if problem.starts_with("sessions"))
    );
}

#[test]
fn the_schema_refuses_invalid_config() {
    let store = Store::open_in_memory().unwrap();
    let connection = store.lock();
    for update in [
        "UPDATE node_config SET hostname = 'Upper.Example'",
        "UPDATE node_config SET http_port = 0",
        "UPDATE node_config SET https_port = http_port",
        "UPDATE node_config SET realm = 'main'",
        "UPDATE node_config SET acme_directory_url = 'http://x'",
        "INSERT INTO node_config (id, realm, http_port, https_port, alias, acme_directory_url) \
         VALUES (2, 'test', 1, 2, '', 'https://x')",
    ] {
        assert!(connection.execute(update, []).is_err(), "{update}");
    }
}

#[test]
fn the_node_key_is_set_once() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.node_key().unwrap().is_none());
    assert!(
        store
            .set_node_key_once(&NodeKey::from_bytes(&[1; 32]), 5)
            .unwrap()
    );
    assert!(
        !store
            .set_node_key_once(&NodeKey::from_bytes(&[2; 32]), 6)
            .unwrap()
    );
    assert_eq!(
        store.node_key().unwrap().map(|key| *key.as_bytes()),
        Some([1; 32])
    );
}

#[test]
fn certificates_and_acme_accounts() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.certificate().unwrap(), None);
    let certificate = StoredCertificate {
        source: CertificateSource::Acme,
        chain_pem: "chain".into(),
        key_pem: "key".into(),
    };
    store.set_certificate(&certificate, 1).unwrap();
    assert_eq!(store.certificate().unwrap(), Some(certificate));
    assert_eq!(store.acme_account("https://a").unwrap(), None);
    store
        .set_acme_account("https://a", "{}", "mailto:x@y", 1)
        .unwrap();
    assert_eq!(
        store.acme_account("https://a").unwrap().as_deref(),
        Some("{}")
    );
}

// D16: the session table is bounded; the least recently used goes first.
#[test]
fn sessions_are_bounded_by_least_recent_use() {
    let store = Store::open_in_memory().unwrap();
    let policy = SessionPolicy {
        max_sessions: 3,
        ..SessionPolicy::DEFAULT
    };
    for (byte, used) in [(1, 40), (2, 10), (3, 30), (4, 20)] {
        store
            .insert_session(&session(byte, 0, used), &policy)
            .unwrap();
    }
    let ids: Vec<u8> = store
        .sessions()
        .unwrap()
        .iter()
        .map(|s| s.id.0[0])
        .collect();
    assert_eq!(ids, [1, 3, 4]);
    store.touch_session(&SessionId([4; 32]), 50).unwrap();
    assert_eq!(store.sessions().unwrap()[0].id, SessionId([4; 32]));
    assert!(store.delete_session(&SessionId([4; 32])).unwrap());
    assert!(!store.delete_session(&SessionId([4; 32])).unwrap());
    assert_eq!(
        store
            .delete_sessions_except(Some(&SessionId([1; 32])))
            .unwrap(),
        1
    );
    assert_eq!(store.sessions().unwrap().len(), 1);
}

#[test]
fn expired_sessions_are_pruned() {
    let store = Store::open_in_memory().unwrap();
    let policy = SessionPolicy::DEFAULT;
    store.insert_session(&session(1, 0, 0), &policy).unwrap();
    store
        .insert_session(&session(2, 0, 20 * DAY_MS), &policy)
        .unwrap();
    store
        .insert_session(&session(3, 0, 400 * DAY_MS), &policy)
        .unwrap();
    assert_eq!(store.prune_sessions(&policy, 30 * DAY_MS).unwrap(), 1);
    assert_eq!(store.prune_sessions(&policy, 365 * DAY_MS).unwrap(), 2);
    assert!(store.sessions().unwrap().is_empty());
}

// A new session is never the one evicted, even when the clock has not
// moved since the others were used.
#[test]
fn a_new_session_survives_eviction_on_a_tie() {
    let store = Store::open_in_memory().unwrap();
    let policy = SessionPolicy {
        max_sessions: 1,
        ..SessionPolicy::DEFAULT
    };
    for byte in [9, 1] {
        store.insert_session(&session(byte, 5, 5), &policy).unwrap();
        let ids: Vec<u8> = store
            .sessions()
            .unwrap()
            .iter()
            .map(|s| s.id.0[0])
            .collect();
        assert_eq!(ids, [byte]);
    }
}
