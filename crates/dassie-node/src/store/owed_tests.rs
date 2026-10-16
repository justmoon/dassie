#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::NodeKey;
use dassie_ledger::{
    AccountId, AccountName, AccountPath, Config, Ledger, LedgerId, Name, Reference, TraceId,
    TransferSpec,
};
use rusqlite::Connection;

use super::schema::{self, APPLICATION_ID};
use super::*;

const TRACE: TraceId = TraceId {
    epoch: 3,
    sequence: 11,
};

fn peer() -> dassie_identity::NodeId {
    NodeKey::from_bytes(&[9; 32]).node_id()
}

/// A ledger with a peer's account and the owner's, committed.
fn accounts(store: &Store) -> (Ledger, AccountId, AccountId) {
    let mut ledger = Ledger::new(Config::DEFAULT);
    let name = Name::new(peer().as_str()).unwrap();
    let (account, _) = ledger.open_peer(1, LedgerId::StubUsd, name, 1_000).unwrap();
    let owner = ledger
        .open(AccountPath::new(
            LedgerId::StubUsd,
            AccountName::EquityOwner,
        ))
        .unwrap();
    commit(store, &mut ledger, LedgerBatch::default());
    (ledger, account, owner)
}

fn commit(store: &Store, ledger: &mut Ledger, mut batch: LedgerBatch) {
    batch.at_ms = 50;
    while let Some(entry) = ledger.poll_journal() {
        batch.journal.push(entry);
    }
    store.commit_ledger(&batch).unwrap();
}

fn owed(account: AccountId, owner: AccountId, fulfill: Vec<u8>) -> OwedFulfill {
    OwedFulfill {
        trace: TRACE,
        peer: peer(),
        request_id: u32::MAX,
        fulfill,
        compensation: Compensation::Reverse,
        transfers: [
            Some((account, owner, 40)),
            Some((owner, account, 1)),
            Some((account, owner, 2)),
        ],
        incoming: Some(IncomingPaid {
            payment: PaymentId([7; 16]),
            connection: ConnectionKey([8; 16]),
            amount: 40,
        }),
        expires_at_ms: 99,
        created_at_ms: 60,
    }
}

// D17: a Fulfill owed a verdict survives a restart as written, the
// largest a Fulfill can be included, and goes once its verdict came.
#[test]
fn owed_fulfills_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let (mut ledger, account, owner) = accounts(&store);
    let largest = dassie_ilp::Fulfill {
        fulfillment: [1; 32],
        data: &vec![2; dassie_ilp::MAX_DATA_LENGTH],
    };
    let mut bytes = vec![0; dassie_ilp::MAX_PACKET_LENGTH];
    let length = dassie_oer::Encode::encode(&largest, &mut bytes);
    bytes.truncate(length);
    let row = owed(account, owner, bytes);
    let mut batch = LedgerBatch::default();
    batch.verdicts.owed.push(row.clone());
    commit(&store, &mut ledger, batch);
    assert_eq!(store.owed_fulfills().unwrap(), [row]);
    let mut batch = LedgerBatch::default();
    batch.verdicts.settled.push(TRACE);
    commit(&store, &mut ledger, batch);
    assert!(store.owed_fulfills().unwrap().is_empty());
}

// D17: a Fulfill taken answers a resend with the same condition only, and
// is forgotten after its time.
#[test]
fn fulfills_taken_answer_resends_until_forgotten() {
    let store = Store::open_in_memory().unwrap();
    let (mut ledger, _, _) = accounts(&store);
    let mut batch = LedgerBatch::default();
    batch.verdicts.taken.push((peer(), 5, [4; 32]));
    commit(&store, &mut ledger, batch);
    assert!(store.fulfill_taken(&peer(), 5, &[4; 32]).unwrap());
    assert!(!store.fulfill_taken(&peer(), 5, &[3; 32]).unwrap());
    assert!(!store.fulfill_taken(&peer(), 6, &[4; 32]).unwrap());
    let mut batch = LedgerBatch::default();
    batch.verdicts.forget_before_ms = Some(51);
    commit(&store, &mut ledger, batch);
    assert!(!store.fulfill_taken(&peer(), 5, &[4; 32]).unwrap());
}

// A refusal is a journal row with the packet's trace, which says what a
// peer's refusals cost since a time: only the losses, not what our own
// receiver took back.
#[test]
fn refusals_are_journaled_per_peer() {
    let store = Store::open_in_memory().unwrap();
    let (mut ledger, account, owner) = accounts(&store);
    let spec = TransferSpec {
        debit: owner,
        credit: account,
        amount: 25,
        pending: false,
        reference: Reference::Refusal(TRACE),
    };
    ledger.create(70, &spec).unwrap();
    commit(&store, &mut ledger, LedgerBatch::default());
    assert_eq!(store.losses_since(account, owner, 70).unwrap(), 25);
    assert_eq!(store.losses_since(account, owner, 71).unwrap(), 0);
    assert_eq!(store.losses_since(account, account, 70).unwrap(), 0);
    let rows = store.transfers_after(None, 10, Some(account)).unwrap();
    let refusal = rows.iter().find(|row| row.transfer.amount == 25).unwrap();
    assert_eq!(refusal.transfer.reference, Reference::Refusal(TRACE));
}

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
