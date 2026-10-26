#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::NodeKey;
use dassie_ledger::{AccountId, Reference};
use rusqlite::Connection;

use super::schema::{self, APPLICATION_ID};
use super::*;

// The journal is rebuilt to take refusals: what it held stays.
#[test]
fn transfers_survive_the_upgrade_to_refusals() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(super::DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 6).unwrap();
    connection
        .execute_batch(
            "INSERT INTO ledger_accounts VALUES (0, 'stub+usd:assets/settlement', 10, 0); \
             INSERT INTO ledger_accounts VALUES (1, 'stub+usd:equity/owner', 0, 10); \
             INSERT INTO ledger_transfers VALUES \
               (0, 0, 1, 10, 'posted', 1, 1, 'deposit', NULL, NULL, NULL, NULL, 1);",
        )
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    let stored = store.load_ledger().unwrap();
    assert_eq!(stored.accounts.len(), 2);
    let rows = store.transfers_after(None, 10, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].transfer.reference, Reference::Deposit);
}

// Migration 43: a refusal a journal still holds becomes a reconciliation,
// with its accounts and amount, so the books are what they were.
#[test]
fn refusals_become_reconciliations_over_the_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(super::DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 42).unwrap();
    connection
        .execute_batch(
            "INSERT INTO ledger_accounts VALUES (0, 'stub+usd:assets/settlement', 10, 4); \
             INSERT INTO ledger_accounts VALUES (1, 'stub+usd:equity/owner', 0, 10); \
             INSERT INTO ledger_accounts VALUES (2, 'stub+usd:expenses/losses', 4, 0); \
             INSERT INTO ledger_transfers VALUES \
               (0, 0, 1, 10, 'posted', 1, 1, 'deposit', NULL, NULL, NULL, NULL, 1), \
               (1, 2, 0, 4, 'posted', 2, 2, 'refusal', 3, 11, NULL, NULL, 2);",
        )
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    let stored = store.load_ledger().unwrap();
    assert_eq!(stored.accounts.len(), 3);
    let rows = store.transfers_after(None, 10, None).unwrap();
    let references: Vec<_> = rows.iter().map(|row| row.transfer.reference).collect();
    assert_eq!(references, [Reference::Deposit, Reference::Reconciliation]);
    assert_eq!(
        (
            rows[1].transfer.debit,
            rows[1].transfer.credit,
            rows[1].transfer.amount
        ),
        (AccountId(2), AccountId(0), 4)
    );
}

// Migration 15: what our receiver took for a Fulfill still owed a verdict
// counts as received, completing a payment it fills; the bans made for a
// grace limit are lifted, the owner's stay; the verdicts' tables go.
#[test]
fn owed_fulfills_count_and_grace_bans_lift_over_the_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 14).unwrap();
    let grace = NodeKey::from_bytes(&[5; 32]).node_id();
    let owner = NodeKey::from_bytes(&[6; 32]).node_id();
    connection
        .execute_batch(&format!(
            "INSERT INTO ledger_accounts VALUES (0, 'stub+usd:assets/interledger/dpeer', 0, 0); \
             INSERT INTO ledger_accounts VALUES (1, 'stub+usd:equity/owner', 0, 0); \
             INSERT INTO incoming_payments VALUES (x'01010101010101010101010101010101', \
             'stub+usd', 'https://a.example/pay', NULL, 100, 40, 'g.a.b', zeroblob(32), NULL, \
             10, 1000, NULL, 1); \
             INSERT INTO incoming_payments VALUES (x'02020202020202020202020202020202', \
             'stub+usd', 'https://a.example/pay', NULL, 50, 20, 'g.a.c', zeroblob(32), NULL, \
             10, 1000, NULL, 2); \
             INSERT INTO incoming_connections VALUES \
               (x'01010101010101010101010101010101', x'08080808080808080808080808080808', 40); \
             INSERT INTO owed_fulfills (packet_epoch, packet_sequence, peer, request_id, \
               fulfill, compensation, debit_1, credit_1, amount_1, incoming_payment, \
               incoming_amount, incoming_connection, expires_at_ms, created_at_ms) VALUES \
               (1, 1, 'dpeer', 7, x'0d', 'reverse', 0, 1, 25, \
                x'01010101010101010101010101010101', 25, x'08080808080808080808080808080808', 90, 60), \
               (1, 2, 'dpeer', 8, x'0d', 'reverse', 0, 1, 30, \
                x'02020202020202020202020202020202', 30, x'09090909090909090909090909090909', 90, 70), \
               (1, 3, 'dpeer', 9, x'0d', 'loss', 0, 1, 5, NULL, NULL, NULL, 90, 80); \
             INSERT INTO peer_bans VALUES ('{grace}'), ('{owner}'); \
             INSERT INTO peer_ban_records VALUES \
               ('{grace}', 'grace-limit', 5, 'stub+usd', 2, 0, 1, NULL), \
               ('{owner}', 'owner', 6, NULL, NULL, NULL, NULL, NULL);",
        ))
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    let open = store.incoming_payment(PaymentId([1; 16])).unwrap().unwrap();
    assert_eq!((open.received_amount, open.completed_at_ms), (65, None));
    let filled = store.incoming_payment(PaymentId([2; 16])).unwrap().unwrap();
    assert_eq!(
        (filled.received_amount, filled.completed_at_ms),
        (50, Some(70))
    );
    assert_eq!(
        store.open_incoming_connections(30, 10).unwrap(),
        [(PaymentId([1; 16]), ConnectionKey([8; 16]), 65)]
    );
    assert_eq!(store.load_peer().unwrap().bans, [owner]);
}
