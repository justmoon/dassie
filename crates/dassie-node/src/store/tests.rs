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

// spec/decisions/database-continuity.md: a database of the early testnet is
// refused with a way forward, and left as it was, for the release that made
// it.
#[test]
fn refuses_a_database_of_the_early_testnet() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(DATABASE_FILE);
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "application_id", EARLY_TESTNET_APPLICATION_ID)
        .unwrap();
    connection.pragma_update(None, "user_version", 49).unwrap();
    connection
        .execute_batch("CREATE TABLE replay_floors (node TEXT PRIMARY KEY) STRICT")
        .unwrap();
    drop(connection);
    let error = Store::open(temp.path()).unwrap_err();
    assert!(matches!(error, StoreError::EarlyTestnetDatabase));
    assert!(error.to_string().contains("set the node up again"));
    let connection = Connection::open(&path).unwrap();
    let application_id: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    assert_eq!(
        (application_id, user_version(&path)),
        (EARLY_TESTNET_APPLICATION_ID, 49)
    );
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

// spec/decisions/release-integrity.md: a rollback needs the database as the
// version before left it.
#[test]
fn keeps_a_copy_before_migrating() {
    let temp = tempfile::tempdir().unwrap();
    let copy = temp.path().join(PRE_MIGRATION_FILE);
    let database = temp.path().join(DATABASE_FILE);
    drop(Store::open(temp.path()).unwrap());
    // A new database needs no copy, nor does one already current.
    drop(Store::open(temp.path()).unwrap());
    assert!(!copy.exists());
    // A version with one more migration keeps a copy before it migrates.
    let next = [
        schema::MIGRATIONS,
        &["UPDATE node_config SET alias = 'next'"],
    ]
    .concat();
    let store = Store::open_with(temp.path(), &next).unwrap();
    assert_eq!(store.config().unwrap().alias.as_str(), "next");
    drop(store);
    assert_eq!(user_version(&copy), SCHEMA_VERSION);
    assert_eq!(mode(&copy), 0o600);
    assert_eq!(user_version(&database), SCHEMA_VERSION + 1);
    // The copy is the database as this version left it.
    fs::rename(&copy, &database).unwrap();
    assert_eq!(
        Store::open(temp.path())
            .unwrap()
            .config()
            .unwrap()
            .alias
            .as_str(),
        ""
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
            .set_node_key_once(&NodeKey::from_bytes(&[1; 32]), Realm::Test, 5)
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
        "UPDATE node_config SET realm = 'main'",
        "UPDATE node_config SET acme_directory_url = 'http://x'",
        "INSERT INTO node_config (id, realm, alias, acme_directory_url) \
         VALUES (2, 'test', '', 'https://x')",
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
            .set_node_key_once(&NodeKey::from_bytes(&[1; 32]), Realm::Test, 5)
            .unwrap()
    );
    assert!(
        !store
            .set_node_key_once(&NodeKey::from_bytes(&[2; 32]), Realm::Test, 6)
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

// A payment may receive more than its amount, as a packet within the
// tolerance may bring it (spec/decisions/paid-within-a-tolerance.md), and
// its connections go with it.
#[test]
fn a_payment_may_receive_more_than_its_amount() {
    let store = Store::open_in_memory().unwrap();
    store
        .lock()
        .execute_batch(
            "INSERT INTO incoming_payments VALUES (x'01010101010101010101010101010101', \
             'stub+usd', 'https://a.example/pay', NULL, 100, 40, 'g.a.b', zeroblob(32), NULL, \
             10, 1000, NULL, 1); \
             INSERT INTO incoming_connections VALUES \
               (x'01010101010101010101010101010101', x'08080808080808080808080808080808', 40);",
        )
        .unwrap();
    assert_eq!(
        store.open_incoming_connections(30, 10).unwrap(),
        [(PaymentId([1; 16]), ConnectionKey([8; 16]), 40)]
    );
    store
        .lock()
        .execute_batch("UPDATE incoming_payments SET received_amount = 101")
        .unwrap();
    let row = store.incoming_payment(PaymentId([1; 16])).unwrap().unwrap();
    assert_eq!((row.incoming_amount, row.received_amount), (Some(100), 101));
    // A payment's connections go with it.
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
        .set_node_key_once(&NodeKey::from_bytes(&[1; 32]), Realm::Test, 5)
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

// spec/areas/rpc.md#paired-devices: the devices are a bounded list, which
// never drops a device to make room.
#[test]
fn devices_are_kept_up_to_their_bound() {
    let store = Store::open_in_memory().unwrap();
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

// spec/areas/hubs.md#in-the-node: a new node keeps no row for any hub, and
// a hub without a row reads as one with nothing on its way and the node's
// own moves on.
#[test]
fn a_hub_without_a_row_reads_as_the_defaults() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.hub_fundings().unwrap().is_empty());
    let xrp = dassie_ledger::Name::new("hub-xrpl-testnet-xrp").unwrap();
    assert_eq!(
        store.hub_funding(xrp).unwrap(),
        (true, HubFundingRow::default())
    );
}

// spec/areas/hubs.md#in-the-node: each hub's moves are kept on their own,
// so one hub's move, pause or choice never touches another's.
#[test]
fn each_hubs_funding_is_kept_on_its_own() {
    let store = Store::open_in_memory().unwrap();
    let xrp = dassie_ledger::Name::new("hub-xrpl-testnet-xrp").unwrap();
    let dsusd = dassie_ledger::Name::new("hub-xrpl-testnet-dsusd").unwrap();
    let xrp_row = HubFundingRow {
        paused_until_ms: 0,
        moving: Some(HubMoveRow {
            deposit: false,
            drops: 1_000_000,
            target: 1_000_100,
            payment: "out-1".into(),
            by_owner: false,
            sent_ms: 9,
            applied_ms: None,
        }),
        given_up: None,
    };
    let dsusd_row = HubFundingRow {
        paused_until_ms: 40,
        moving: None,
        given_up: Some((true, 5)),
    };
    store
        .commit_ledger(&LedgerBatch {
            at_ms: 1,
            hub_automatic: [(dsusd, false)].into(),
            hub_funding: [(xrp, xrp_row.clone()), (dsusd, dsusd_row.clone())].into(),
            ..LedgerBatch::default()
        })
        .unwrap();
    assert_eq!(store.hub_funding(xrp).unwrap(), (true, xrp_row.clone()));
    assert_eq!(store.hub_funding(dsusd).unwrap(), (false, dsusd_row));
    // The XRP hub's move done, and its own moves turned off: the DSUSD
    // hub's stay as they were.
    let done = HubFundingRow {
        moving: None,
        ..xrp_row
    };
    store
        .commit_ledger(&LedgerBatch {
            at_ms: 2,
            hub_automatic: [(xrp, false)].into(),
            hub_funding: [(xrp, done.clone())].into(),
            ..LedgerBatch::default()
        })
        .unwrap();
    let all = store.hub_fundings().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[&xrp], (false, done));
    assert_eq!(all[&dsusd].1.given_up, Some((true, 5)));
    assert!(!all[&dsusd].0);
}

// spec/areas/node.md#database, spec/decisions/settlement-methods.md: a
// database written before a peering's methods had a table of their own
// moves forward through migration 2. Its peering loads as one method, on
// the scheme it was made on, with its state, credit and time short of
// room; what is the pair's stays, the termination being told included, and
// nothing of it cascades away while `peers` is rebuilt.
#[test]
fn a_peering_stored_before_methods_loads_as_its_one_method() {
    let temp = tempfile::tempdir().unwrap();
    let node = dassie_identity::NodeKey::from_bytes(&[9; 32]).node_id();
    {
        let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        schema::prepare(&mut connection, &schema::MIGRATIONS[..1]).unwrap();
        connection
            .execute_batch(&format!(
                "INSERT INTO peer_nodes (id, link_state, sources, first_seen_ms)
                   VALUES ('{node}', NULL, 1, 10);
                 INSERT INTO peers (node, scheme, state, credit_out, since_ms, inbound,
                   last_seen_ms, short_for_ms, depeering_since_ms, depeering_reason,
                   short_reached_ms)
                   VALUES ('{node}', 'stub', x'0102', 900, 14, 1, 15, 17, 18, 4, 12);
                 INSERT INTO peer_terminations (node, since_ms, reason, by_us, acknowledged)
                   VALUES ('{node}', 20, 1, 1, 0);"
            ))
            .unwrap();
    }
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        user_version(&temp.path().join(DATABASE_FILE)),
        SCHEMA_VERSION
    );
    let stored = store.load_peer().unwrap();
    let [loaded] = stored.nodes.as_slice() else {
        panic!("{:?}", stored.nodes)
    };
    let peering = loaded.peering.as_ref().unwrap();
    assert_eq!(
        peering.methods,
        [dassie_peer::Method {
            scheme: dassie_peer::wire::SchemeId::parse("stub").unwrap(),
            state: vec![1, 2],
            credit_out: 900,
            short_for_ms: 17,
            short_reached_ms: 12,
            short_checked_ms: None,
            winding: None,
            unpayable_since_ms: None,
        }]
    );
    assert_eq!(
        (peering.since_ms, peering.inbound, peering.last_seen_ms),
        (14, true, 15)
    );
    assert_eq!(peering.depeering.map(|d| d.since_ms), Some(18));
    let termination = loaded.termination.unwrap();
    assert_eq!((termination.since_ms, termination.by_us), (20, true));
    // The peering's row still holds its termination's and its method's:
    // removing the node takes all three.
    store
        .commit_ledger(&LedgerBatch {
            peer: dassie_peer::PeerBatch {
                removed: vec![node],
                ..dassie_peer::PeerBatch::default()
            },
            ..LedgerBatch::default()
        })
        .unwrap();
    for table in ["peers", "peer_methods", "peer_terminations"] {
        let rows: i64 = store
            .lock()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
}
