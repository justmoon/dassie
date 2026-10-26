#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use dassie_kit::DAY_MS;
use dassie_ledger::LedgerId;
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

// The database is private to its owner whatever the umask.
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
fn one_store_at_a_time_opens_a_data_directory() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    assert!(matches!(Store::open(temp.path()), Err(StoreError::InUse)));
    assert_eq!(mode(&temp.path().join(LOCK_FILE)), 0o600);
    drop(store);
    drop(Store::open(temp.path()).unwrap());
    // Stores in memory share nothing.
    let _one = Store::open_in_memory().unwrap();
    let _two = Store::open_in_memory().unwrap();
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

// spec/decisions/database-continuity.md: the original's database is refused
// with a way forward.
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
            known: SCHEMA_VERSION
        })
    ));
}

fn user_version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/// A database at schema version `version`, as an older Dassie left it.
fn database_at(dir: &Path, version: usize) {
    let mut connection = Connection::open(dir.join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, version).unwrap();
}

// spec/areas/settlement.md#external-assets: migration 45 gives each XRPL
// account the config its schemes named of it, the first of them by ID, and
// leaves the schemes whether they peer and nothing else.
#[test]
fn migration_45_gives_each_account_its_own_config() {
    let temp = tempfile::tempdir().unwrap();
    database_at(temp.path(), 44);
    let connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    let schemes = [
        ("stub", r#"{"trust":"unlimited"}"#),
        (
            "xrpl-testnet",
            r#"{"server":"ws://a","peers":false,"faucet":null}"#,
        ),
        ("xrpl-testnet-dsusd", r#"{"server":"ws://b"}"#),
        ("xrpl", r#"{"peers": true}"#),
        ("xrpl-rlusd", ""),
    ];
    for (id, config) in schemes {
        connection
            .execute(
                "INSERT INTO settlement_schemes (id, config, activated_at_ms) VALUES (?1, ?2, 7)",
                [id, config],
            )
            .unwrap();
    }
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store.asset_configs().unwrap(),
        [(
            "xrpl-testnet-account/0".to_string(),
            r#"{"server":"ws://a","faucet":null}"#.to_string()
        )]
    );
    let configs: Vec<(String, String)> = store
        .settlement_schemes()
        .unwrap()
        .into_iter()
        .map(|(id, config)| (id.as_str().to_string(), config))
        .collect();
    let expected = [
        ("stub", r#"{"trust":"unlimited"}"#),
        ("xrpl", ""),
        ("xrpl-rlusd", ""),
        ("xrpl-testnet", r#"{"peers":false}"#),
        ("xrpl-testnet-dsusd", ""),
    ];
    assert_eq!(
        configs,
        expected.map(|(id, config)| (id.to_string(), config.to_string()))
    );
}

// spec/decisions/release-integrity.md: a rollback needs the database as the
// version before left it.
#[test]
fn keeps_a_copy_before_migrating() {
    let temp = tempfile::tempdir().unwrap();
    let copy = temp.path().join(PRE_MIGRATION_FILE);
    drop(Store::open(temp.path()).unwrap());
    // A new database needs no copy, nor does one already current.
    drop(Store::open(temp.path()).unwrap());
    assert!(!copy.exists());
    fs::remove_file(temp.path().join(DATABASE_FILE)).unwrap();
    let older = usize::try_from(SCHEMA_VERSION - 1).unwrap();
    database_at(temp.path(), older);
    drop(Store::open(temp.path()).unwrap());
    assert_eq!(user_version(&copy), SCHEMA_VERSION - 1);
    assert_eq!(mode(&copy), 0o600);
    assert_eq!(
        user_version(&temp.path().join(DATABASE_FILE)),
        SCHEMA_VERSION
    );
}

/// Starts a newer version on `dir` as far as its migration: it keeps a copy
/// of the database, migrates past this version, and changes the hostname.
fn newer_version_migrates(dir: &Path) {
    let connection = Connection::open(dir.join(DATABASE_FILE)).unwrap();
    snapshot::keep_copy(&connection, dir, SCHEMA_VERSION).unwrap();
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    connection
        .execute("UPDATE node_config SET hostname = 'newer.example'", [])
        .unwrap();
}

#[test]
fn a_rollback_puts_the_copy_back() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let mut config = store.config().unwrap();
    config.hostname = Some(Hostname::parse("older.example").unwrap());
    store.set_config(&config).unwrap();
    drop(store);
    // This version can open what it left: nothing to do.
    let since = std::time::SystemTime::now();
    assert_eq!(
        restore_pre_migration(temp.path(), since).unwrap(),
        Restored::Nothing
    );
    newer_version_migrates(temp.path());
    assert!(matches!(
        Store::open(temp.path()),
        Err(StoreError::TooNew { .. })
    ));
    assert_eq!(
        restore_pre_migration(temp.path(), since).unwrap(),
        Restored::Copy {
            from: SCHEMA_VERSION + 1,
            to: SCHEMA_VERSION
        }
    );
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(store.config().unwrap(), config);
    assert!(!temp.path().join(PRE_MIGRATION_FILE).exists());
    // What the newer version left stays, for the owner to look at.
    assert_eq!(
        user_version(&temp.path().join(ROLLED_BACK_FILE)),
        SCHEMA_VERSION + 1
    );
    assert!(!temp.path().join(ROLLING_BACK_FILE).exists());
    // Nothing a finished rollback leaves stops the owner starting afresh.
    drop(store);
    fs::remove_file(temp.path().join(DATABASE_FILE)).unwrap();
    drop(Store::open(temp.path()).unwrap());
}

// spec/decisions/release-integrity.md: a node never starts afresh where a
// rollback stopped half way, with another identity and no books.
#[test]
fn opens_no_new_database_where_a_rollback_did_not_finish() {
    let temp = tempfile::tempdir().unwrap();
    drop(Store::open(temp.path()).unwrap());
    newer_version_migrates(temp.path());
    // What a rollback leaves if it stops between moving the database aside
    // and putting the copy in.
    let database = temp.path().join(DATABASE_FILE);
    fs::rename(&database, temp.path().join(ROLLING_BACK_FILE)).unwrap();
    assert!(matches!(
        Store::open(temp.path()),
        Err(StoreError::RollbackUnfinished)
    ));
    assert!(!database.exists());
    // Nor does another rollback take what this one moved aside.
    assert_eq!(
        restore_pre_migration(temp.path(), std::time::SystemTime::UNIX_EPOCH).unwrap(),
        Restored::Nothing
    );
    assert!(temp.path().join(ROLLING_BACK_FILE).exists());
    // Once the owner puts the copy back, the node opens it.
    fs::rename(temp.path().join(PRE_MIGRATION_FILE), &database).unwrap();
    drop(Store::open(temp.path()).unwrap());
}

#[test]
fn a_rollback_refuses_a_copy_it_cannot_trust() {
    let temp = tempfile::tempdir().unwrap();
    drop(Store::open(temp.path()).unwrap());
    newer_version_migrates(temp.path());
    // A copy from before this update began is from another migration.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    assert!(matches!(
        restore_pre_migration(temp.path(), later),
        Err(StoreError::NoUsableCopy(_))
    ));
    fs::remove_file(temp.path().join(PRE_MIGRATION_FILE)).unwrap();
    let since = std::time::SystemTime::UNIX_EPOCH;
    assert!(matches!(
        restore_pre_migration(temp.path(), since),
        Err(StoreError::NoUsableCopy(_))
    ));
    // Nothing moved: the database is as the newer version left it.
    assert_eq!(
        user_version(&temp.path().join(DATABASE_FILE)),
        SCHEMA_VERSION + 1
    );
}

#[test]
fn the_setup_token_lasts_until_setup() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.setup_token().unwrap(), None);
    store.set_setup_token(&[3; 32]).unwrap();
    assert_eq!(store.setup_token().unwrap(), Some([3; 32]));
    assert!(
        store
            .set_node_key_once(&NodeKey::from_bytes(&[1; 32]), 5)
            .unwrap()
    );
    assert_eq!(store.setup_token().unwrap(), None);
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

// spec/decisions/tigerstyle.md: the session table is bounded; the least
// recently used goes first.
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

// Migration 5 rebuilds the token table: tokens from before keep everything,
// revoked or not, and have no cap.
#[test]
fn tokens_survive_the_upgrade_to_caps() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 4).unwrap();
    connection
        .execute_batch(
            "INSERT INTO btp_tokens VALUES (1, zeroblob(32), 'old', 'stub+usd', 5, NULL); \
             INSERT INTO btp_tokens VALUES (2, randomblob(32), 'gone', 'stub+usd', 6, 7);",
        )
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    let tokens = store.btp_tokens().unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!((tokens[0].label.as_str(), tokens[0].cap), ("old", None));
    assert_eq!(tokens[1].revoked_at_ms, Some(7));
}

// Migration 13 keeps what connections received apart: an open incoming
// payment's credentials from before are one connection, which received
// all it did; a closed one's connections are not read again.
#[test]
fn open_payments_keep_what_they_received_over_the_upgrade_to_connections() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 12).unwrap();
    connection
        .execute_batch(
            "INSERT INTO incoming_payments VALUES (x'01010101010101010101010101010101', \
             'stub+usd', 'https://a.example/pay', NULL, 100, 40, 'g.a.b', zeroblob(32), NULL, \
             10, 1000, NULL, 1); \
             INSERT INTO incoming_payments VALUES (x'02020202020202020202020202020202', \
             'stub+usd', 'https://a.example/pay', NULL, 10, 10, 'g.a.c', zeroblob(32), NULL, \
             10, 1000, 20, 2);",
        )
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store.open_incoming_connections(30, 10).unwrap(),
        [(PaymentId([1; 16]), ConnectionKey::LEGACY, 40)]
    );
}

// Migration 28 lets a payment receive more than its amount, as a packet
// within the tolerance may bring it
// (spec/decisions/paid-within-a-tolerance.md), and keeps the payments and
// what their connections received.
#[test]
fn payments_and_their_connections_survive_the_upgrade_to_a_tolerance() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    schema::migrate_until(&mut connection, 27).unwrap();
    connection
        .execute_batch(
            "INSERT INTO incoming_payments VALUES (x'01010101010101010101010101010101', \
             'stub+usd', 'https://a.example/pay', NULL, 100, 40, 'g.a.b', zeroblob(32), NULL, \
             10, 1000, NULL, 1); \
             INSERT INTO incoming_connections VALUES \
               (x'01010101010101010101010101010101', x'08080808080808080808080808080808', 40);",
        )
        .unwrap();
    let over = "UPDATE incoming_payments SET received_amount = 101";
    assert!(connection.execute_batch(over).is_err());
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    let key = ConnectionKey([8; 16]);
    assert_eq!(
        store.open_incoming_connections(30, 10).unwrap(),
        [(PaymentId([1; 16]), key, 40)]
    );
    store.lock().execute_batch(over).unwrap();
    let row = store.incoming_payment(PaymentId([1; 16])).unwrap().unwrap();
    assert_eq!((row.incoming_amount, row.received_amount), (Some(100), 101));
    // A payment's connections still go with it.
    store
        .lock()
        .execute_batch("DELETE FROM incoming_payments")
        .unwrap();
    assert!(store.open_incoming_connections(30, 10).unwrap().is_empty());
}

// Found by the simulation (spec/decisions/random-seed-simulations.md): a
// receiver that restarts after an incoming payment completed must still tell
// a sender that reconnects what its connection delivered, or the sender,
// having lost a Fulfill to a crash, gives up on money that arrived. A
// completed payment's connections are kept for a while, then pruned; an open
// one's stay.
#[test]
fn completed_payments_keep_what_their_connections_received_for_a_while() {
    let store = Store::open_in_memory().unwrap();
    store
        .lock()
        .execute_batch(
            "INSERT INTO incoming_payments VALUES (x'01010101010101010101010101010101', \
             'stub+usd', 'https://a.example/pay', NULL, 100, 100, 'g.a.b', zeroblob(32), NULL, \
             10, 1000000000, 500, 1); \
             INSERT INTO incoming_payments VALUES (x'02020202020202020202020202020202', \
             'stub+usd', 'https://a.example/pay', NULL, 100, 40, 'g.a.c', zeroblob(32), NULL, \
             10, 1000000000, NULL, 2); \
             INSERT INTO incoming_payments VALUES (x'03030303030303030303030303030303', \
             'stub+usd', 'https://a.example/pay', NULL, NULL, 10, 'g.a.d', zeroblob(32), NULL, \
             10, 1000000000, 300, 3); \
             INSERT INTO incoming_connections VALUES \
               (x'01010101010101010101010101010101', x'08080808080808080808080808080808', 60), \
               (x'01010101010101010101010101010101', x'09090909090909090909090909090909', 40), \
               (x'02020202020202020202020202020202', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', 40);",
        )
        .unwrap();
    let first = FinishedIncoming {
        id: PaymentId([1; 16]),
        ledger: LedgerId::StubUsd,
        full: true,
        received: vec![(ConnectionKey([8; 16]), 60), (ConnectionKey([9; 16]), 40)],
    };
    // Completed by its creator, without an amount: never full.
    let third = FinishedIncoming {
        id: PaymentId([3; 16]),
        ledger: LedgerId::StubUsd,
        full: false,
        received: Vec::new(),
    };
    // Newest first, open ones left out, and only since a time.
    assert_eq!(
        store.finished_incoming(0, 10).unwrap(),
        [first.clone(), third]
    );
    assert_eq!(
        store.finished_incoming(400, 10).unwrap(),
        std::slice::from_ref(&first)
    );
    assert_eq!(
        store.finished_incoming(0, 1).unwrap(),
        std::slice::from_ref(&first)
    );
    store
        .prune_incoming(500 + FINISHED_RETENTION_MS - 1)
        .unwrap();
    assert_eq!(store.finished_incoming(0, 1).unwrap(), [first]);
    store.prune_incoming(500 + FINISHED_RETENTION_MS).unwrap();
    assert_eq!(store.finished_incoming(0, 1).unwrap()[0].received, []);
    assert_eq!(
        store
            .open_incoming_connections(500 + FINISHED_RETENTION_MS, 10)
            .unwrap(),
        [(PaymentId([2; 16]), ConnectionKey([10; 16]), 40)]
    );
}

// The inspector shows every table but never reads a secret
// (spec/areas/rpc.md#the-inspector).
#[test]
fn the_inspector_never_reads_secrets() {
    let store = Store::open_in_memory().unwrap();
    store
        .set_node_key_once(&NodeKey::from_bytes(&[1; 32]), 5)
        .unwrap();
    let certificate = StoredCertificate {
        source: CertificateSource::Manual,
        chain_pem: "c".repeat(5_000),
        key_pem: "secret".into(),
    };
    store.set_certificate(&certificate, 1).unwrap();
    store
        .insert_session(&session(3, 1, 1), &SessionPolicy::DEFAULT)
        .unwrap();
    let tables = store.inspect_tables().unwrap();
    assert_eq!(tables.len(), schema::TABLES.len());
    let node_key = tables.iter().find(|t| t.name == "node_key").unwrap();
    assert_eq!(node_key.rows, 1);
    assert!(node_key.columns[1].redacted);
    let rows = |table| store.inspect_rows(table, 0, 10).unwrap().unwrap().rows;
    assert_eq!(rows("node_key")[0][1].value, Value::Redacted);
    assert_eq!(rows("sessions")[0][0].value, Value::Redacted);
    let tls = &rows("tls_certificate")[0];
    assert_eq!(tls[3].value, Value::Redacted);
    assert_eq!(tls[2].full_length, Some(5_000));
    assert!(matches!(&tls[2].value, Value::Text(text) if text.len() == TEXT_CUT));
    assert!(
        store
            .inspect_rows("sqlite_master", 0, 10)
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_inspector_pages_through_tables() {
    let store = Store::open_in_memory().unwrap();
    for byte in 0..25 {
        store
            .insert_session(&session(byte, 1, 1), &SessionPolicy::DEFAULT)
            .unwrap();
    }
    let mut cursor = 0;
    let mut seen = 0;
    loop {
        let page = store.inspect_rows("sessions", cursor, 10).unwrap().unwrap();
        seen += page.rows.len();
        match page.next_cursor {
            Some(next) => cursor = next,
            None => break,
        }
    }
    assert_eq!(seen, 25);
    let page = store.inspect_rows("node_config", 0, 10).unwrap().unwrap();
    assert_eq!((page.rows.len(), page.next_cursor), (1, None));
}

// Any column named like a secret must be classified, so that a new one
// is not shown by accident.
#[test]
fn columns_named_like_secrets_are_classified() {
    const PUBLIC: &[(&str, &str)] = &[
        ("settlement_channels", "public_key"),
        // A client's public key, as its JWKS publishes it.
        ("gnap_grants", "client_key"),
    ];
    for table in schema::TABLES {
        for column in table.columns {
            let name = column.0;
            let suspicious = ["key", "secret", "token", "credential", "password", "hash"]
                .iter()
                .any(|word| name.contains(word));
            let classified = schema::SECRET_COLUMNS
                .iter()
                .chain(PUBLIC)
                .any(|entry| *entry == (table.name, name));
            assert!(!suspicious || classified, "{}.{name}", table.name);
        }
    }
}

fn quote_row(n: u32, client: Option<&str>, now: u64) -> QuoteRow {
    let mut id = [0u8; 16];
    id[..4].copy_from_slice(&n.to_be_bytes());
    id[4] = u8::from(client.is_some());
    QuoteRow {
        id: PaymentId(id),
        ledger: LedgerId::StubUsd,
        wallet_address: "https://localhost/pay/stub".into(),
        client: client.map(String::from),
        receiver: "https://elsewhere.example/incoming-payments/1".into(),
        destination: "test.elsewhere".into(),
        shared_secret: [1; 32],
        debit_amount: 1,
        receive_amount: ForeignAmount {
            value: 1,
            asset_code: "USD".into(),
            asset_scale: 9,
        },
        created_at_ms: now,
        expires_at_ms: now + 60_000,
        recipient: client
            .is_none()
            .then(|| "https://elsewhere.example/.well-known/pay".into()),
        fixed_debit: client.is_some(),
    }
}

// Found in review: clients' quotes, which need no owner, could fill the
// one cap and leave the owner's Send none.
#[test]
fn clients_quotes_do_not_crowd_out_the_owners() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let now = 1_800_000_000_000;
    let client = Some("jwk:x");
    for n in 0..MAX_LIVE_QUOTES {
        assert!(store.insert_quote(&quote_row(n, client, now), now).unwrap());
    }
    let one_more = quote_row(MAX_LIVE_QUOTES, client, now);
    assert!(!store.insert_quote(&one_more, now).unwrap());
    for n in 0..MAX_LIVE_OWNER_QUOTES {
        assert!(store.insert_quote(&quote_row(n, None, now), now).unwrap());
    }
    let one_more = quote_row(MAX_LIVE_OWNER_QUOTES, None, now);
    assert!(!store.insert_quote(&one_more, now).unwrap());
    // Each keeps what it pays; expired, they make room again.
    let kept = store.quote(quote_row(0, None, now).id).unwrap().unwrap();
    assert_eq!(
        kept.recipient.as_deref(),
        Some("https://elsewhere.example/.well-known/pay")
    );
    let later = now + 60_000;
    assert!(
        store
            .insert_quote(&quote_row(0, None, later), later)
            .unwrap()
    );
}

// Migration 19: nothing is settled at a peering's end
// (spec/decisions/ending-a-peering.md). A final settlement still due when the
// node upgrades is dropped with its columns; the channel, and what it owes
// the peer on its accounts, stay.
#[test]
fn a_final_settlement_due_is_dropped_over_the_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    database_at(temp.path(), 18);
    let key = dassie_identity::NodeKey::from_bytes(&[3; 32]);
    Connection::open(temp.path().join(DATABASE_FILE))
        .unwrap()
        .execute(
            "INSERT INTO settlement_channels (node, public_key, url, settled_in, settled_out,
               final_scheme, final_state)
             VALUES (?1, ?2, 'https://c.example', 4, 7, 'stub', x'04')",
            rusqlite::params![key.node_id().as_str(), key.public_key().as_bytes().to_vec()],
        )
        .unwrap();
    let store = Store::open(temp.path()).unwrap();
    let channels = store.load_peer().unwrap().channels;
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].node, key.node_id());
    assert_eq!(
        (
            channels[0].channel.settled_in,
            channels[0].channel.settled_out
        ),
        (4, 7)
    );
}

// Migration 25: a peer's replay floor moves out of its peering into the
// table of floors, which every sender kept for good has
// (spec/areas/peer-protocol.md#replay-protection).
#[test]
fn a_peers_replay_floor_survives_the_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    database_at(temp.path(), 24);
    let key = dassie_identity::NodeKey::from_bytes(&[3; 32]);
    let connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .execute(
            "INSERT INTO peer_nodes (id, sources, first_seen_ms) VALUES (?1, 0, 1)",
            [key.node_id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO peers (node, scheme, version, state, credit_out,
               since_ms, inbound, last_seen_ms, replay_floor_ms)
             VALUES (?1, 'stub', 1, x'', 0, 2, 0, 3, 1800000000123)",
            [key.node_id().as_str()],
        )
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store.load_peer().unwrap().floors,
        vec![dassie_peer::StoredFloor {
            node: key.node_id(),
            floor_ms: 1_800_000_000_123,
        }]
    );
}

// spec/areas/rpc.md#paired-devices: a database from before devices gains
// their table, which keeps a bounded list and never drops a device to make
// room.
#[test]
fn devices_are_kept_after_the_upgrade_up_to_their_bound() {
    let temp = tempfile::tempdir().unwrap();
    database_at(temp.path(), 26);
    let store = Store::open(temp.path()).unwrap();
    let device = |seed: u8, paired_at_ms: u64| DeviceRow {
        id: dassie_identity::NodeKey::from_bytes(&[seed; 32]).node_id(),
        name: format!("device {seed}"),
        paired_at_ms,
        last_seen_at_ms: paired_at_ms,
    };
    for seed in 0..u8::try_from(MAX_DEVICES).unwrap() {
        store.add_device(&device(seed, u64::from(seed))).unwrap();
    }
    assert!(matches!(
        store.add_device(&device(200, 1_000)),
        Err(AddDevice::Full)
    ));
    // Pairing a device again is no new device.
    store.add_device(&device(0, 2_000)).unwrap();
    let listed = store.devices().unwrap();
    assert_eq!(listed.len(), usize::try_from(MAX_DEVICES).unwrap());
    assert_eq!(listed[0], device(0, 2_000));
    let first = device(0, 0).id;
    store.touch_device(&first, 5_000).unwrap();
    store.touch_device(&first, 4_000).unwrap();
    assert_eq!(
        store.device(&first).unwrap().unwrap().last_seen_at_ms,
        5_000
    );
    assert!(store.delete_device(&first).unwrap());
    assert!(!store.delete_device(&first).unwrap());
    assert_eq!(store.device(&first).unwrap(), None);
    store.add_device(&device(200, 3_000)).unwrap();
}

/// Grants as a store at version 35 kept them, the `n`th with ID, revision
/// and continuation hash of `n`, asked for at 10 and decided at 20; each
/// approved one has a token whose hash is of `n + 100`. 1 and 6 to 9 create
/// under a receive limit alone, 1 and 8 approved, 6 pending, 9 denied; 2
/// to 4 create under a debit limit, both or none; 5 only reads under a
/// receive limit; 7 creates incoming payments.
fn grants_at_v35(dir: &Path) {
    use serde_json::{Value, json};

    database_at(dir, 35);
    let usd = json!({ "value": "5", "assetCode": "USD", "assetScale": 9 });
    let outgoing = |actions: Value, limits: Value| {
        json!({
            "type": "outgoing-payment",
            "actions": actions,
            "identifier": "https://a.example/.well-known/pay",
            "limits": limits,
        })
    };
    let create = json!(["create", "read"]);
    let receive = json!({ "receiveAmount": usd });
    let receive_alone = outgoing(create.clone(), receive.clone());
    let debit_alone = outgoing(create.clone(), json!({ "debitAmount": usd }));
    let both = outgoing(
        create.clone(),
        json!({ "debitAmount": usd, "receiveAmount": usd }),
    );
    let no_amount = outgoing(create, json!({ "interval": "R/2026-01-01T00:00:00Z/P1D" }));
    let read_only = outgoing(json!(["read", "list"]), receive);
    let incoming = json!({ "type": "incoming-payment", "actions": ["create", "read"] });
    let quote = json!({ "type": "quote", "actions": ["create"] });
    let grants: [(u8, &str, Value); 9] = [
        (1, "approved", json!([receive_alone])),
        (2, "approved", json!([debit_alone])),
        (3, "approved", json!([both])),
        (4, "approved", json!([no_amount])),
        (5, "approved", json!([read_only])),
        (6, "pending", json!([receive_alone])),
        (7, "approved", json!([incoming])),
        (8, "approved", json!([quote, receive_alone])),
        (9, "denied", json!([receive_alone])),
    ];
    let connection = Connection::open(dir.join(DATABASE_FILE)).unwrap();
    for (n, state, access) in &grants {
        connection
            .execute(
                "INSERT INTO gnap_grants (id, state, client, client_key, access, needs_owner,
                   continue_token_hash, interaction, created_at_ms, decided_at_ms, expires_at_ms,
                   revision)
                 VALUES (?1, ?2, 'https://client.example', '{}', ?3, 1, ?4, NULL, 10, ?5, 100, ?6)",
                rusqlite::params![
                    vec![*n; 16],
                    state,
                    access.to_string(),
                    vec![*n; 32],
                    (*state != "pending").then_some(20),
                    i64::from(*n),
                ],
            )
            .unwrap();
        if *state == "approved" {
            connection
                .execute(
                    "INSERT INTO gnap_tokens VALUES (?1, ?2, ?2, 20, 1000)",
                    rusqlite::params![vec![*n + 100; 32], vec![*n; 16]],
                )
                .unwrap();
        }
    }
}

// Migration 36 (spec/areas/payments.md#what-a-grant-allows): a grant kept
// from before that creates outgoing payments under a receive limit alone is
// revoked, pending or approved, as the owner revokes one: its token stops
// working, its client is refused, and the owner no longer sees it. Other
// limits, items that only read, and other access types stay as they were.
#[test]
fn grants_that_limit_only_what_is_received_are_revoked_on_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    grants_at_v35(temp.path());
    let store = Store::open(temp.path()).unwrap();
    let state_of = |n: u8| store.grant(PaymentId([n; 16])).unwrap().unwrap().state;
    let token_works = |n: u8| store.access_token(&[n + 100; 32], 30).unwrap().is_some();
    for n in [1, 8] {
        assert_eq!(state_of(n), GrantState::Revoked, "grant {n}");
        assert!(!token_works(n), "grant {n}");
        // Its client, continuing it, is told it was not approved.
        let continued = store.continued_grant(PaymentId([n; 16]), &[n; 32]).unwrap();
        assert_eq!(continued.unwrap().state, GrantState::Revoked);
    }
    for n in [2, 3, 4, 5, 7] {
        assert_eq!(state_of(n), GrantState::Approved, "grant {n}");
        assert!(token_works(n), "grant {n}");
    }
    let pending = store.grant(PaymentId([6; 16])).unwrap().unwrap();
    assert_eq!(
        (pending.state, pending.decided_at_ms),
        (GrantState::Revoked, Some(10))
    );
    assert_eq!(state_of(9), GrantState::Denied);
    // The owner sees only the grants still approved.
    let mut listed: Vec<u8> = store
        .consented_grants(16)
        .unwrap()
        .iter()
        .map(|grant| grant.id.0[0])
        .collect();
    listed.sort_unstable();
    assert_eq!(listed, [2, 3, 4, 5, 7]);
    // Each grant revoked has a revision of its own, past every other.
    let revisions: Vec<i64> = store
        .lock()
        .prepare("SELECT revision FROM gnap_grants WHERE state = 'revoked' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(revisions, [10, 11, 12]);
    // They go at the next pruning, as revoked grants do.
    store.prune_grants(30).unwrap();
    assert_eq!(store.grant(PaymentId([1; 16])).unwrap(), None);
}
