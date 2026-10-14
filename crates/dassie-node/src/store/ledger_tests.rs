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
    let rows = store.transfers_after(None, 100, None).unwrap();
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
        .transfers_after(Some(rows[0].sequence), 1, None)
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].transfer.id, rows[1].transfer.id);
}

#[test]
fn a_batch_is_all_or_nothing() {
    let store = Store::open_in_memory().unwrap();
    let mut ledger = scenario(&store);
    let before = store.transfers_after(None, 100, None).unwrap();
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
        cap: None,
        cap_usage: dassie_connector::CapUsage::default(),
    });
    assert!(store.commit_ledger(&batch).is_err());
    assert_eq!(store.transfers_after(None, 100, None).unwrap(), before);
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
        cap: None,
        cap_usage: dassie_connector::CapUsage::default(),
    };
    let batch = LedgerBatch {
        at_ms: 10,
        schemes_added: vec![(stub, "{}".into())],
        checkpoints: vec![(
            stub,
            Checkpoint {
                balance: 100,
                state: vec![1, 2],
            },
        )],
        tokens_created: vec![token.clone()],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    assert_eq!(store.settlement_schemes().unwrap(), [(stub, "{}".into())]);
    assert_eq!(store.btp_tokens().unwrap(), std::slice::from_ref(&token));
    let batch = LedgerBatch {
        at_ms: 20,
        schemes_removed: vec![stub],
        checkpoints: vec![(
            stub,
            Checkpoint {
                balance: 150,
                state: Vec::new(),
            },
        )],
        tokens_revoked: vec![1],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    assert!(store.settlement_schemes().unwrap().is_empty());
    // The checkpoint outlives the scheme, so adding it again books nothing
    // twice (ACC-6).
    assert_eq!(
        store.checkpoints().unwrap(),
        [(
            stub,
            Checkpoint {
                balance: 150,
                state: Vec::new(),
            }
        )]
    );
    assert_eq!(store.btp_tokens().unwrap()[0].revoked_at_ms, Some(20));
}

// Found in review: paging by transfer ID missed a packet transfer posted
// after one with a larger ID had been read.
#[test]
fn paging_sees_transfers_posted_out_of_order() {
    let store = Store::open_in_memory().unwrap();
    let mut ledger = scenario(&store);
    let owner = ledger.find(&path(AccountName::EquityOwner)).unwrap();
    let token = ledger
        .find(&path(AccountName::EquityOwnerSub(OwnerSub::Btp(1))))
        .unwrap();
    let spec = |sequence| TransferSpec {
        debit: token,
        credit: owner,
        amount: 2,
        pending: true,
        reference: Reference::Packet(TraceId { epoch: 4, sequence }),
    };
    let slow = ledger.create(10, &spec(20)).unwrap();
    let fast = ledger.create(10, &spec(21)).unwrap();
    ledger.post(11, fast).unwrap();
    store.commit_ledger(&drain(&mut ledger, 11)).unwrap();
    let seen = store.transfers_after(None, 100, None).unwrap();
    let last = seen.last().unwrap();
    assert_eq!(last.transfer.id, fast);
    let cursor = last.sequence;
    ledger.post(12, slow).unwrap();
    store.commit_ledger(&drain(&mut ledger, 12)).unwrap();
    let next = store.transfers_after(Some(cursor), 100, None).unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].transfer.id, slow);
    assert!(slow < fast);
}

// Found in review: removing a scheme and adding it again in one batch
// broke the commit, and so the engine.
#[test]
fn a_scheme_removed_and_added_in_one_batch_stays() {
    let store = Store::open_in_memory().unwrap();
    let stub = Name::new("stub").unwrap();
    let added = LedgerBatch {
        schemes_added: vec![(stub, String::new())],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&added).unwrap();
    let again = LedgerBatch {
        schemes_removed: vec![stub],
        schemes_added: vec![(stub, "{}".into())],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&again).unwrap();
    assert_eq!(store.settlement_schemes().unwrap(), [(stub, "{}".into())]);
}

// Spec 7 §4.5: a token's cap and what is used of it survive exactly, the
// usage split into whole amounts and a remainder.
#[test]
fn token_caps_round_trip_exactly() {
    let store = Store::open_in_memory().unwrap();
    let cap = dassie_connector::SpendingCap {
        amount: 1_000,
        window_ms: 3_600_000,
    };
    let usage = dassie_connector::CapUsage {
        used: 123 * 3_600_000 + 4_567,
        as_of_ms: 99,
    };
    let token = BtpTokenRow {
        id: 1,
        token_hash: [7; 32],
        label: "capped".into(),
        ledger: LedgerId::StubUsd,
        created_at_ms: 10,
        revoked_at_ms: None,
        cap: Some(cap),
        cap_usage: dassie_connector::CapUsage::new(10),
    };
    let batch = LedgerBatch {
        at_ms: 10,
        tokens_created: vec![token.clone()],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    assert_eq!(store.btp_tokens().unwrap(), std::slice::from_ref(&token));
    let batch = LedgerBatch {
        at_ms: 99,
        token_caps: vec![TokenCap {
            id: 1,
            cap: Some(cap),
            usage,
        }],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    let stored = &store.btp_tokens().unwrap()[0];
    assert_eq!((stored.cap, stored.cap_usage), (Some(cap), usage));
    let batch = LedgerBatch {
        at_ms: 100,
        token_caps: vec![TokenCap {
            id: 1,
            cap: None,
            usage: dassie_connector::CapUsage::new(100),
        }],
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    assert_eq!(store.btp_tokens().unwrap()[0].cap, None);
}

// The wallet's account page reads one account's transfers, newest last,
// and pages on from there.
#[test]
fn transfers_of_one_account() {
    let store = Store::open_in_memory().unwrap();
    let ledger = scenario(&store);
    let token = ledger
        .find(&path(AccountName::EquityOwnerSub(OwnerSub::Btp(1))))
        .unwrap();
    let settlement = ledger.find(&path(AccountName::AssetsSettlement)).unwrap();
    let all = store.transfers_after(None, 100, None).unwrap();
    let of = |account| {
        all.iter()
            .filter(|row| row.transfer.debit == account || row.transfer.credit == account)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        store.transfers_after(None, 100, Some(token)).unwrap(),
        of(token)
    );
    assert_eq!(
        store.transfers_after(None, 100, Some(settlement)).unwrap(),
        of(settlement)
    );
    // Both sides of an account in one page, in order and bounded.
    let owner = ledger.find(&path(AccountName::EquityOwner)).unwrap();
    let first = store.transfers_after(None, 1, Some(owner)).unwrap();
    assert_eq!(first, of(owner)[..1]);
    let rest = store
        .transfers_after(Some(first[0].sequence), 100, Some(owner))
        .unwrap();
    assert_eq!(rest, of(owner)[1..]);
    let last = store.transfers_last(1, Some(owner)).unwrap();
    assert_eq!(last, of(owner)[of(owner).len() - 1..]);
    assert_eq!(store.transfers_last(100, None).unwrap(), all);
}
