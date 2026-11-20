//! The schema against what the releases wrote
//! (spec/decisions/database-continuity.md): `testdata/v0.4.3/` holds the
//! thirteen migrations Dassie 0.4.3 ran, as it shipped them. 0.3.0 ran the
//! first nine, the same text, 0.3.1 to 0.3.3 the first ten, 0.3.4 to
//! 0.4.1 the first eleven, and 0.4.2 the first twelve. A database
//! the first nine build, as 0.3.0 built one, is the one the baseline
//! builds; the migrations after the baseline are the released ones; and a
//! database each release left opens, and migrates to this version's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use rusqlite::Connection;
use rusqlite::types::Value;

use super::schema::{self, BASELINE_VERSION, MIGRATIONS};
use super::{APPLICATION_ID, DATABASE_FILE, PRE_MIGRATION_FILE, SCHEMA_VERSION, Store};

/// The migrations Dassie 0.4.3 ran, in order, as it shipped them.
const RELEASED: [&str; 13] = [
    include_str!("testdata/v0.4.3/01.sql"),
    include_str!("testdata/v0.4.3/02.sql"),
    include_str!("testdata/v0.4.3/03.sql"),
    include_str!("testdata/v0.4.3/04.sql"),
    include_str!("testdata/v0.4.3/05.sql"),
    include_str!("testdata/v0.4.3/06.sql"),
    include_str!("testdata/v0.4.3/07.sql"),
    include_str!("testdata/v0.4.3/08.sql"),
    include_str!("testdata/v0.4.3/09.sql"),
    include_str!("testdata/v0.4.3/10.sql"),
    include_str!("testdata/v0.4.3/11.sql"),
    include_str!("testdata/v0.4.3/12.sql"),
    include_str!("testdata/v0.4.3/13.sql"),
];

/// Each release since the testnet's reset, at the version its database
/// is: the number of the released migrations it ran.
const RELEASES: [(&str, i64); 5] = [
    ("0.3.0", 9),
    ("0.3.3", 10),
    ("0.3.4", 11),
    ("0.4.2", 12),
    ("0.4.3", 13),
];

/// Builds the database at `path` as a release that ran the first
/// `version` released migrations did: its application ID, then each
/// migration in its own transaction, which sets `user_version` to its
/// number.
fn build_as_released(path: &Path, version: i64) {
    let mut connection = Connection::open(path).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    let ran = usize::try_from(version).unwrap();
    for (index, sql) in RELEASED[..ran].iter().enumerate() {
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch(sql).unwrap();
        transaction
            .pragma_update(None, "user_version", i64::try_from(index + 1).unwrap())
            .unwrap();
        transaction.commit().unwrap();
    }
}

/// `sql` without comments, identifier quotes or layout, so that two
/// statements that mean the same compare equal: SQLite keeps a table's
/// text as written, with what `ALTER TABLE` changed spliced in.
fn normalized(sql: &str) -> String {
    let uncommented: String = sql
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join(" ");
    let words: Vec<&str> = uncommented.split_whitespace().collect();
    let mut text = words.join(" ").replace('"', "");
    for punctuation in ["(", ")", ","] {
        text = text
            .replace(&format!(" {punctuation}"), punctuation)
            .replace(&format!("{punctuation} "), punctuation);
    }
    text
}

fn rows(connection: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = connection.prepare(sql).unwrap();
    let width = statement.column_count();
    statement
        .query_map([], |row| {
            (0..width).map(|i| row.get::<_, Value>(i)).collect()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Everything SQLite says of the database: its application ID and version;
/// each object's text, normalized; each table's flags, columns, foreign
/// keys and indexes, and each index's columns, as SQLite reads them; and
/// every row of every table.
fn described(connection: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    let mut description = vec![(
        "pragmas".to_string(),
        rows(
            connection,
            "SELECT * FROM pragma_application_id, pragma_user_version",
        ),
    )];
    let objects = rows(
        connection,
        "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name",
    );
    description.push((
        "sqlite_schema".to_string(),
        objects
            .iter()
            .map(|object| {
                let mut object = object.clone();
                if let Some(Value::Text(sql)) = object.last_mut() {
                    *sql = normalized(sql);
                }
                object
            })
            .collect(),
    ));
    for object in &objects {
        let (Value::Text(kind), Value::Text(name)) = (&object[0], &object[1]) else {
            panic!("{object:?}");
        };
        let queries = match kind.as_str() {
            "table" => vec![
                format!(
                    "SELECT type, ncol, wr, strict FROM pragma_table_list \
                     WHERE schema = 'main' AND name = '{name}'"
                ),
                format!("SELECT * FROM pragma_table_xinfo('{name}')"),
                format!("SELECT * FROM pragma_foreign_key_list('{name}')"),
                format!("SELECT * FROM pragma_index_list('{name}') ORDER BY name"),
                format!("SELECT * FROM \"{name}\" ORDER BY 1"),
            ],
            "index" => vec![format!("SELECT * FROM pragma_index_xinfo('{name}')")],
            _ => panic!("an object the schema does not use: {object:?}"),
        };
        for query in queries {
            let found = rows(connection, &query);
            description.push((query, found));
        }
    }
    description
}

/// A database that `migrations` of this version's built, as `migrate`
/// builds one, at `path`.
fn build(path: &Path, migrations: &[&str]) {
    let mut connection = Connection::open(path).unwrap();
    schema::prepare(&mut connection, migrations).unwrap();
}

/// This version's migrations that bring a database to `version`.
fn up_to(version: i64) -> &'static [&'static str] {
    &MIGRATIONS[..usize::try_from(version - BASELINE_VERSION + 1).unwrap()]
}

fn assert_alike(old: &[(String, Vec<Vec<Value>>)], new: &[(String, Vec<Vec<Value>>)]) {
    for ((query, old), (_, new)) in old.iter().zip(new) {
        assert_eq!(old, new, "{query}");
    }
    assert_eq!(old.len(), new.len());
}

// The baseline builds what Dassie 0.3.0's nine migrations built: the same
// application ID and version, tables, columns, constraints, indexes and
// foreign keys, and the same rows a new node starts with.
#[test]
fn the_baseline_is_what_the_first_release_wrote() {
    let temp = tempfile::tempdir().unwrap();
    let (released, baseline) = (temp.path().join("released"), temp.path().join("baseline"));
    build_as_released(&released, BASELINE_VERSION);
    build(&baseline, &MIGRATIONS[..1]);
    let released = described(&Connection::open(&released).unwrap());
    let baseline = described(&Connection::open(&baseline).unwrap());
    assert_eq!(
        released[0].1,
        [[
            Value::Integer(APPLICATION_ID.into()),
            Value::Integer(BASELINE_VERSION)
        ]]
    );
    assert_alike(&released, &baseline);
}

// The comparison sees what an edit to the baseline would change: a row it
// seeds, a column's default, a table's ROWID.
#[test]
fn the_comparison_sees_an_edited_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let released = temp.path().join("released");
    build_as_released(&released, BASELINE_VERSION);
    let released = described(&Connection::open(&released).unwrap());
    let baseline = MIGRATIONS[0];
    let seeded = format!("{baseline}\nUPDATE peer_settings SET swapping = 0;");
    let edits = [
        seeded,
        baseline.replacen("DEFAULT 1", "DEFAULT 0", 1),
        baseline.replacen(", WITHOUT ROWID", "", 1),
    ];
    for (index, edit) in edits.iter().enumerate() {
        assert_ne!(edit, baseline);
        let path = temp.path().join(format!("edited{index}"));
        build(&path, &[edit]);
        assert_ne!(
            described(&Connection::open(&path).unwrap()),
            released,
            "edit {index}"
        );
    }
}

// The migrations after the baseline start with the released ones,
// verbatim, as the releases after the first ran them; any after those are
// this version's, not released yet.
#[test]
fn the_migrations_after_the_baseline_are_the_released_ones() {
    let after = usize::try_from(BASELINE_VERSION).unwrap();
    let released = &RELEASED[after..];
    assert_eq!(&MIGRATIONS[1..=released.len()], released);
    assert!(SCHEMA_VERSION >= i64::try_from(RELEASED.len()).unwrap());
}

// spec/decisions/database-continuity.md: the database each release left
// opens. One at an older version than this one's, as 0.3.0's at 9,
// 0.3.3's at 10 and 0.3.4's at 11 are, migrates to it, keeping a copy as
// it was, and ends as a new database is; one at this version opens as it
// is, with no copy kept.
#[test]
fn every_releases_database_opens() {
    let temp = tempfile::tempdir().unwrap();
    let fresh = temp.path().join("fresh");
    build(&fresh, MIGRATIONS);
    let fresh = described(&Connection::open(&fresh).unwrap());
    for (release, version) in RELEASES {
        let dir = temp.path().join(release);
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(DATABASE_FILE);
        build_as_released(&path, version);
        let before = described(&Connection::open(&path).unwrap());
        drop(Store::open(&dir).unwrap());
        let after = described(&Connection::open(&path).unwrap());
        let copy = dir.join(PRE_MIGRATION_FILE);
        if version < SCHEMA_VERSION {
            let kept = described(&Connection::open(&copy).unwrap());
            assert_alike(&before, &kept);
        } else {
            assert!(
                !copy.exists(),
                "{release}: a current database keeps no copy"
            );
            assert_alike(&before, &after);
        }
        assert_alike(&fresh, &after);
    }
}

// spec/areas/hubs.md#the-validator-in-the-node: migration 15 keeps one
// verifier's state per network, the one kept beside the newest cursor of
// that network's hubs, which its first four bytes name; a hub that kept
// none, or none that reads as a network's, is left out, and every hub
// keeps its cursor in a row without the verifier's column.
#[test]
fn migration_15_keeps_each_networks_newest_verifier() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v14");
    build(&path, up_to(14));
    let connection = Connection::open(&path).unwrap();
    let state = |network: u32, tail: u8| {
        let mut state = network.to_be_bytes().to_vec();
        state.push(tail);
        state
    };
    let escrows: [(u8, u32, i64, Option<Vec<u8>>); 6] = [
        (1, 50, 900, Some(state(1, 1))),
        (2, 60, 1_000, Some(state(1, 2))),
        (3, 70, 800, Some(state(1, 3))),
        (4, 10, 500, Some(state(0, 4))),
        (5, 99, 9_000, None),
        (6, 99, 9_000, Some(vec![1, 2])),
    ];
    for (hub, ledger, close_time, verifier) in &escrows {
        connection
            .execute(
                "INSERT INTO hub_escrow \
                 (hub, newest_ledger, newest_close_time, scanned_to, verifier) \
                 VALUES (?1, ?2, ?3, 0, ?4)",
                rusqlite::params![vec![*hub; 32], ledger, close_time, verifier],
            )
            .unwrap();
    }
    drop(connection);
    build(&path, MIGRATIONS);
    let mut connection = Connection::open(&path).unwrap();
    let blob = |bytes: &[u8]| Value::Blob(bytes.to_vec());
    assert_eq!(
        rows(
            &connection,
            "SELECT network, updated_at_ms, verifier FROM hub_verifiers ORDER BY network"
        ),
        [
            vec![
                blob(&0_u32.to_be_bytes()),
                Value::Integer(0),
                blob(&state(0, 4))
            ],
            vec![
                blob(&1_u32.to_be_bytes()),
                Value::Integer(0),
                blob(&state(1, 2))
            ],
        ]
    );
    let cursors = rows(&connection, "SELECT * FROM hub_escrow ORDER BY hub");
    assert_eq!(cursors.len(), escrows.len(), "every cursor stays");
    assert!(cursors.iter().all(|row| row.len() == 4), "{cursors:?}");
    schema::check(&mut connection).unwrap();
}

// spec/areas/connector.md#sources: migration 16 keeps the owner's rate
// sources as they were, in their order, and admits the central banks
// beside the ECB, and an XRP Ledger pool with a URL; a URL on any other
// kind, or a kind it does not know, is still refused.
#[test]
fn migration_16_widens_the_rate_sources_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v15");
    build(&path, up_to(15));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DELETE FROM rate_sources;
             INSERT INTO rate_sources VALUES
               (0, 'ecb', NULL), (1, 'coinbase', 'https://localhost:10100/rates.json');",
        )
        .unwrap();
    assert!(
        connection
            .execute("INSERT INTO rate_sources VALUES (2, 'cnb', NULL)", [])
            .is_err(),
        "not a kind before"
    );
    drop(connection);
    build(&path, MIGRATIONS);
    let mut connection = Connection::open(&path).unwrap();
    let text = |text: &str| Value::Text(text.into());
    assert_eq!(
        rows(&connection, "SELECT * FROM rate_sources ORDER BY position"),
        [
            vec![Value::Integer(0), text("ecb"), Value::Null],
            vec![
                Value::Integer(1),
                text("coinbase"),
                text("https://localhost:10100/rates.json")
            ],
        ]
    );
    for row in [
        "(2, 'norges-bank', NULL)",
        "(3, 'bank-of-canada', NULL)",
        "(4, 'cnb', NULL)",
        "(5, 'xrpl-pool', NULL)",
        "(6, 'xrpl-pool', 'wss://xrplcluster.com')",
    ] {
        connection
            .execute(&format!("INSERT INTO rate_sources VALUES {row}"), [])
            .unwrap();
    }
    for row in [
        "(7, 'cnb', 'https://example.com')",
        "(7, 'boc', NULL)",
        "(8, 'ecb', NULL)",
    ] {
        assert!(
            connection
                .execute(&format!("INSERT INTO rate_sources VALUES {row}"), [])
                .is_err(),
            "{row}"
        );
    }
    schema::check(&mut connection).unwrap();
}

// spec/decisions/open-rate-sources.md: migration 17 moves a node still on
// the sources it started with to a new node's, the pool and the four
// central banks; any other list, an empty one or the stock list with one
// source changed, stays as the owner left it.
#[test]
fn migration_17_moves_the_stock_sources_to_the_open_ones() {
    let text = |text: &str| Value::Text(text.into());
    let open = vec![
        vec![Value::Integer(0), text("xrpl-pool"), Value::Null],
        vec![Value::Integer(1), text("ecb"), Value::Null],
        vec![Value::Integer(2), text("norges-bank"), Value::Null],
        vec![Value::Integer(3), text("bank-of-canada"), Value::Null],
        vec![Value::Integer(4), text("cnb"), Value::Null],
    ];
    let stock =
        "(0, 'coinbase', NULL), (1, 'kraken', NULL), (2, 'bitstamp', NULL), (3, 'ecb', NULL)";
    let cases: [(&str, Option<Vec<Vec<Value>>>); 5] = [
        (stock, Some(open)),
        ("", None),
        (
            "(0, 'coinbase', NULL), (1, 'kraken', NULL), (2, 'bitstamp', NULL), (3, 'ecb', NULL), (4, 'cnb', NULL)",
            None,
        ),
        (
            "(0, 'coinbase', 'https://localhost:10100/rates.json'), (1, 'kraken', NULL), (2, 'bitstamp', NULL), (3, 'ecb', NULL)",
            None,
        ),
        (
            "(0, 'kraken', NULL), (1, 'coinbase', NULL), (2, 'bitstamp', NULL), (3, 'ecb', NULL)",
            None,
        ),
    ];
    for (at, (list, after)) in cases.into_iter().enumerate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(format!("v16-{at}"));
        build(&path, up_to(16));
        let connection = Connection::open(&path).unwrap();
        connection.execute("DELETE FROM rate_sources", []).unwrap();
        if !list.is_empty() {
            connection
                .execute(&format!("INSERT INTO rate_sources VALUES {list}"), [])
                .unwrap();
        }
        let before = rows(&connection, "SELECT * FROM rate_sources ORDER BY position");
        drop(connection);
        build(&path, MIGRATIONS);
        let mut connection = Connection::open(&path).unwrap();
        let now = rows(&connection, "SELECT * FROM rate_sources ORDER BY position");
        assert_eq!(now, after.unwrap_or(before), "{list}");
        schema::check(&mut connection).unwrap();
    }
}
