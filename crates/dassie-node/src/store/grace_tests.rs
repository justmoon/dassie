#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::NodeKey;
use dassie_ledger::LedgerId;
use rusqlite::Connection;

use super::*;
use crate::store::{LedgerBatch, schema};

fn node(seed: u8) -> dassie_identity::NodeId {
    NodeKey::from_bytes(&[seed; 32]).node_id()
}

fn commit(store: &Store, batch: LedgerBatch) {
    store.commit_ledger(&batch).unwrap();
}

#[test]
fn the_peer_settings_start_at_the_default_and_change() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.peer_settings().unwrap(), PeerSettings::DEFAULT);
    let settings = PeerSettings {
        grace_share_ppm: 2_500_000,
    };
    commit(
        &store,
        LedgerBatch {
            peer_settings: Some(settings),
            ..LedgerBatch::default()
        },
    );
    assert_eq!(store.peer_settings().unwrap(), settings);
    // The table refuses what the code would.
    let refused = store.commit_ledger(&LedgerBatch {
        peer_settings: Some(PeerSettings { grace_share_ppm: 0 }),
        ..LedgerBatch::default()
    });
    assert!(refused.is_err());
    assert_eq!(store.peer_settings().unwrap(), settings);
}

fn commit_bans(store: &Store, bans: Vec<BanChange>) {
    commit(
        store,
        LedgerBatch {
            bans,
            ..LedgerBatch::default()
        },
    );
}

const COST: PeerCost = PeerCost {
    ledger: LedgerId::StubUsd,
    refused: 600,
    overdue: 0,
    grace: 550,
};

fn automatic(seed: u8) -> BanRecord {
    BanRecord {
        node: node(seed),
        reason: BanReason::GraceLimit,
        banned_at_ms: 10,
        cost: Some(COST),
        unbanned_at_ms: None,
    }
}

fn by_owner(seed: u8) -> BanRecord {
    BanRecord {
        node: node(seed),
        reason: BanReason::Owner,
        banned_at_ms: 11,
        cost: None,
        unbanned_at_ms: None,
    }
}

// A record outlives its ban: lifting it keeps when, so what the peer cost
// before does not count again.
#[test]
fn ban_records_keep_why_and_when() {
    let store = Store::open_in_memory().unwrap();
    commit_bans(
        &store,
        vec![
            BanChange::Banned(automatic(1)),
            BanChange::Banned(by_owner(2)),
        ],
    );
    let mut records = store.ban_records().unwrap();
    records.sort_by_key(|record| record.banned_at_ms);
    assert_eq!(records, [automatic(1), by_owner(2)]);

    commit_bans(
        &store,
        vec![BanChange::Unbanned {
            node: node(1),
            at_ms: 20,
        }],
    );
    let lifted = store
        .ban_records()
        .unwrap()
        .into_iter()
        .find(|record| record.node == node(1))
        .unwrap();
    assert_eq!(lifted.unbanned_at_ms, Some(20));
    assert_eq!(lifted.cost, Some(COST));
}

// A new ban replaces the record; a record forgotten is gone.
#[test]
fn a_new_ban_replaces_its_record() {
    let store = Store::open_in_memory().unwrap();
    commit_bans(
        &store,
        vec![
            BanChange::Banned(automatic(1)),
            BanChange::Banned(by_owner(2)),
            BanChange::Unbanned {
                node: node(1),
                at_ms: 20,
            },
        ],
    );
    let again = BanRecord {
        banned_at_ms: 30,
        ..by_owner(1)
    };
    commit_bans(
        &store,
        vec![BanChange::Banned(again), BanChange::Forgotten(node(2))],
    );
    assert_eq!(store.ban_records().unwrap(), [again]);
}

// A ban over the grace limit says what the peer cost, and only such a ban
// does.
#[test]
fn only_an_automatic_ban_carries_a_cost() {
    let store = Store::open_in_memory().unwrap();
    let refused = store.commit_ledger(&LedgerBatch {
        bans: vec![BanChange::Banned(BanRecord {
            node: node(1),
            reason: BanReason::GraceLimit,
            banned_at_ms: 1,
            cost: None,
            unbanned_at_ms: None,
        })],
        ..LedgerBatch::default()
    });
    assert!(refused.is_err());
}

// Migration 10 adds the tables to a database of migration 9, with the
// default share.
#[test]
fn the_grace_limit_comes_with_an_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(super::DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", schema::APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 9).unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(store.peer_settings().unwrap(), PeerSettings::DEFAULT);
    assert!(store.ban_records().unwrap().is_empty());
}
