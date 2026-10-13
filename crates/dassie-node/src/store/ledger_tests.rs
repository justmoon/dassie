#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_ledger::{
    AccountName, AccountPath, Config, Ledger, LedgerId, Name, OwnerSub, Reference, TraceId,
    TransferSpec, TransferState,
};

use super::*;

fn path(name: AccountName) -> AccountPath {
    AccountPath::new(LedgerId::StubUsd, name)
}

fn drain(ledger: &mut Ledger, at_ms: u64) -> LedgerBatch {
    let mut batch = LedgerBatch {
        at_ms,
        ..LedgerBatch::default()
    };
    while let Some(entry) = ledger.poll_journal() {
        batch.journal.push(entry);
    }
    batch
}

fn restore(store: &Store) -> Ledger {
    let stored = store.load_ledger().unwrap();
    Ledger::restore(
        Config::DEFAULT,
        stored.accounts,
        stored.pending,
        stored.next_transfer,
    )
    .unwrap()
}

/// A ledger with a deposit, a posted packet, a packet in flight and a
/// pending settlement, committed.
fn scenario(store: &Store) -> Ledger {
    let mut ledger = Ledger::new(Config::DEFAULT);
    let settlement = ledger.open(path(AccountName::AssetsSettlement)).unwrap();
    let owner = ledger.open(path(AccountName::EquityOwner)).unwrap();
    let token = ledger
        .open(path(AccountName::EquityOwnerSub(OwnerSub::Btp(1))))
        .unwrap();
    let spec = |debit, credit, amount, pending, reference| TransferSpec {
        debit,
        credit,
        amount,
        pending,
        reference,
    };
    ledger
        .create(
            1,
            &spec(settlement, owner, 1_000, false, Reference::Deposit),
        )
        .unwrap();
    store.commit_ledger(&drain(&mut ledger, 1)).unwrap();
    let packet = Reference::Packet(TraceId {
        epoch: 4,
        sequence: 9,
    });
    let id = ledger
        .create(2, &spec(token, owner, 30, true, packet))
        .unwrap();
    ledger.post(3, id).unwrap();
    ledger
        .create(4, &spec(token, owner, 5, true, packet))
        .unwrap();
    let settlement_reference = Reference::Settlement {
        scheme: Name::new("stub").unwrap(),
        id: Name::new("abc123").unwrap(),
    };
    ledger
        .create(5, &spec(owner, settlement, 7, true, settlement_reference))
        .unwrap();
    store.commit_ledger(&drain(&mut ledger, 5)).unwrap();
    ledger
}

#[test]
fn the_journal_restores_the_ledger() {
    let store = Store::open_in_memory().unwrap();
    let ledger = scenario(&store);
    let restored = restore(&store);
    restored.check_invariants();
    for (id, account) in ledger.accounts() {
        let other = restored.account(id).unwrap();
        assert_eq!(other.path(), account.path());
        assert_eq!(
            other.balance().net_credit(),
            account.balance().net_credit(),
            "{}",
            account.path()
        );
    }
    // The settlement is still pending; the packet in flight is gone.
    let pending: Vec<_> = restored.pending_transfers().collect();
    assert_eq!(pending.len(), 1);
    assert!(matches!(pending[0].reference, Reference::Settlement { .. }));
    assert_eq!(restored.next_transfer_id(), ledger.next_transfer_id());
}

#[test]
fn journal_rows_carry_their_references_and_paths() {
    let store = Store::open_in_memory().unwrap();
    scenario(&store);
    let rows = store.transfers_after(None, 100).unwrap();
    assert_eq!(
        rows.len(),
        3,
        "the deposit, the posted packet, the settlement"
    );
    assert_eq!(rows[0].transfer.reference, Reference::Deposit);
    assert_eq!(rows[1].state, TransferState::Posted);
    assert_eq!(rows[1].resolved_at_ms, Some(3));
    assert_eq!(rows[1].debit_path, "stub+usd:equity/owner/btp/1");
    assert_eq!(rows[1].credit_path, "stub+usd:equity/owner");
    assert_eq!(
        rows[1].transfer.reference,
        Reference::Packet(TraceId {
            epoch: 4,
            sequence: 9
        })
    );
    assert_eq!(rows[2].state, TransferState::Pending);
    let after = store
        .transfers_after(Some(rows[0].transfer.id.0), 1)
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].transfer.id, rows[1].transfer.id);
}

#[test]
fn a_batch_is_all_or_nothing() {
    let store = Store::open_in_memory().unwrap();
    let mut ledger = scenario(&store);
    let before = store.transfers_after(None, 100).unwrap();
    let owner = ledger.find(&path(AccountName::EquityOwner)).unwrap();
    let suspense = ledger.open(path(AccountName::EquitySuspense)).unwrap();
    ledger
        .create(
            6,
            &TransferSpec {
                debit: suspense,
                credit: owner,
                amount: 3,
                pending: false,
                reference: Reference::Reconciliation,
            },
        )
        .unwrap();
    let mut batch = drain(&mut ledger, 6);
    // A token row that breaks a constraint sinks the whole batch.
    batch.tokens_created.push(BtpTokenRow {
        id: 0,
        token_hash: [1; 32],
        label: String::new(),
        ledger: LedgerId::StubUsd,
        created_at_ms: 6,
        revoked_at_ms: None,
    });
    assert!(store.commit_ledger(&batch).is_err());
    assert_eq!(store.transfers_after(None, 100).unwrap(), before);
    assert_eq!(store.load_ledger().unwrap().accounts.len(), 3);
}

#[test]
fn totals_that_disagree_with_the_journal_are_refused() {
    let store = Store::open_in_memory().unwrap();
    scenario(&store);
    store
        .lock()
        .execute(
            "UPDATE ledger_accounts SET credits_posted = credits_posted + 1 WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(matches!(
        store.load_ledger(),
        Err(StoreError::Corrupt("account total"))
    ));
}

#[test]
fn boot_epochs_count_up() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.next_boot_epoch().unwrap(), 1);
    assert_eq!(store.next_boot_epoch().unwrap(), 2);
}

#[test]
fn schemes_checkpoints_and_tokens_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let stub = Name::new("stub").unwrap();
    let token = BtpTokenRow {
        id: 1,
        token_hash: [7; 32],
        label: "wallet".into(),
        ledger: LedgerId::StubUsd,
        created_at_ms: 10,
        revoked_at_ms: None,
    };
    let batch = LedgerBatch {
        at_ms: 10,
        schemes_added: vec![(stub, "{}".into())],
        checkpoints: vec![(stub, 100)],
        tokens_created: vec![token.clone()],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    assert_eq!(store.settlement_schemes().unwrap(), [(stub, "{}".into())]);
    assert_eq!(store.btp_tokens().unwrap(), std::slice::from_ref(&token));
    let batch = LedgerBatch {
        at_ms: 20,
        schemes_removed: vec![stub],
        checkpoints: vec![(stub, 150)],
        tokens_revoked: vec![1],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    assert!(store.settlement_schemes().unwrap().is_empty());
    // The checkpoint outlives the scheme, so adding it again books nothing
    // twice (ACC-6).
    assert_eq!(store.checkpoints().unwrap(), [(stub, 150)]);
    assert_eq!(store.btp_tokens().unwrap()[0].revoked_at_ms, Some(20));
}
