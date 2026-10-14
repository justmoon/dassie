//! The ledger's behaviour, and the original's gaps it closes (spec 5 §10).

mod support;

use dassie_ledger::{
    AccountName, Config, JournalEntry, Ledger, LedgerError, LedgerId, Name, OwnerSub, Reference,
    RestoreError, TraceId, TransferError, TransferId, TransferSpec, TransferState,
};
use support::{Mirror, net, open, path};

const UNIT: u64 = 1_000_000_000;

fn packet(sequence: u64) -> Reference {
    Reference::Packet(TraceId { epoch: 1, sequence })
}

fn spec(
    debit: dassie_ledger::AccountId,
    credit: dassie_ledger::AccountId,
    amount: u64,
    pending: bool,
    reference: Reference,
) -> TransferSpec {
    TransferSpec {
        debit,
        credit,
        amount,
        pending,
        reference,
    }
}

/// A ledger with the owner funded through a deposit of `funds`.
fn funded(funds: u64) -> (Ledger, Accounts) {
    let mut ledger = Ledger::new(Config::DEFAULT);
    let accounts = Accounts {
        settlement: open(&mut ledger, AccountName::AssetsSettlement),
        owner: open(&mut ledger, AccountName::EquityOwner),
        token: open(&mut ledger, AccountName::EquityOwnerSub(OwnerSub::Btp(1))),
        suspense: open(&mut ledger, AccountName::EquitySuspense),
    };
    if funds > 0 {
        ledger
            .create(
                0,
                &spec(
                    accounts.settlement,
                    accounts.owner,
                    funds,
                    false,
                    Reference::Deposit,
                ),
            )
            .unwrap();
    }
    (ledger, accounts)
}

struct Accounts {
    settlement: dassie_ledger::AccountId,
    owner: dassie_ledger::AccountId,
    token: dassie_ledger::AccountId,
    suspense: dassie_ledger::AccountId,
}

fn peer() -> Name {
    Name::new("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW").unwrap()
}

// ACC-4: opening an account again reset its pending counters.
#[test]
fn opening_an_open_account_changes_nothing() {
    let (mut ledger, accounts) = funded(UNIT);
    ledger
        .create(1, &spec(accounts.token, accounts.owner, 5, true, packet(0)))
        .unwrap();
    let before = *ledger.account(accounts.token).unwrap();
    let again = ledger
        .open(path(AccountName::EquityOwnerSub(OwnerSub::Btp(1))))
        .unwrap();
    assert_eq!(again, accounts.token);
    assert_eq!(*ledger.account(accounts.token).unwrap(), before);
    ledger.check_invariants();
}

// ACC-8: debit and credit on the same account was an assertion, reachable
// by a BTP client paying the node's own receiver.
#[test]
fn invalid_transfers_are_values() {
    let (mut ledger, accounts) = funded(UNIT);
    let other = ledger
        .open(dassie_ledger::AccountPath::new(
            LedgerId::XrplTestnetXrp,
            AccountName::EquityOwner,
        ))
        .unwrap();
    let cases = [
        (
            spec(accounts.owner, accounts.owner, 1, true, packet(0)),
            TransferError::SameAccount,
        ),
        (
            spec(accounts.token, accounts.owner, 0, true, packet(0)),
            TransferError::ZeroAmount,
        ),
        (
            spec(accounts.owner, other, 1, true, packet(0)),
            TransferError::DifferentLedgers,
        ),
        (
            spec(
                accounts.owner,
                dassie_ledger::AccountId(999),
                1,
                true,
                packet(0),
            ),
            TransferError::UnknownAccount,
        ),
    ];
    for (spec, error) in cases {
        assert_eq!(ledger.create(1, &spec), Err(error));
    }
    ledger.check_invariants();
}

// ACC-9: the owner's equity had no limit.
#[test]
fn the_owner_cannot_spend_more_than_the_pool() {
    let (mut ledger, accounts) = funded(100);
    let (peer, _) = ledger.open_peer(0, LedgerId::StubUsd, peer(), 0).unwrap();
    ledger
        .create(1, &spec(accounts.token, peer, 60, true, packet(0)))
        .unwrap();
    // 60 pending out of 100: 41 more is too much, also from the pool itself.
    assert_eq!(
        ledger.create(1, &spec(accounts.token, peer, 41, true, packet(1))),
        Err(TransferError::ExceedsDebits)
    );
    assert_eq!(
        ledger.create(1, &spec(accounts.owner, peer, 41, true, packet(1))),
        Err(TransferError::ExceedsDebits)
    );
    ledger
        .create(1, &spec(accounts.owner, peer, 40, true, packet(1)))
        .unwrap();
    assert_eq!(ledger.stats().refused, 2);
    ledger.check_invariants();
}

// Paying the node's own receiver moves money within the group: it needs no
// free funds, and the journal still records it (spec 7 §4.5).
#[test]
fn transfers_within_the_group_need_no_funds() {
    let (mut ledger, accounts) = funded(0);
    let id = ledger
        .create(
            1,
            &spec(accounts.token, accounts.owner, 7 * UNIT, true, packet(0)),
        )
        .unwrap();
    ledger.post(2, id).unwrap();
    assert_eq!(net(&ledger, accounts.token), -7 * i128::from(UNIT));
    assert_eq!(net(&ledger, accounts.owner), 7 * i128::from(UNIT));
    let group = ledger.group_balance(accounts.owner).unwrap();
    assert_eq!(group.net_credit(), 0);
    let mut mirror = Mirror::default();
    mirror.drain(&mut ledger);
    let (_, state) = mirror.rows[&id];
    assert_eq!(state, TransferState::Posted);
    ledger.check_invariants();
}

// ACC-7: a cross-ledger packet whose second transfer failed left the first
// pending for ever.
#[test]
fn linked_transfers_are_all_or_none() {
    let (mut ledger, accounts) = funded(10);
    let (peer, _) = ledger.open_peer(0, LedgerId::StubUsd, peer(), 0).unwrap();
    let mut ids = Vec::new();
    let refused = ledger.create_linked(
        1,
        &[
            spec(accounts.token, accounts.owner, 5, true, packet(0)),
            spec(peer, accounts.settlement, 5, true, packet(0)),
        ],
        &mut ids,
    );
    // The peer's interledger account has no credit to draw on.
    assert_eq!(refused, Err((1, TransferError::ExceedsDebits)));
    assert!(ids.is_empty());
    assert_eq!(ledger.pending_transfers().count(), 0);
    assert_eq!(ledger.stats().voided, 1);
    ledger.check_invariants();
}

// ACC-5: the trust line was posted again at every start.
#[test]
fn peer_trust_is_extended_once() {
    let mut ledger = Ledger::new(Config::DEFAULT);
    let (first, new) = ledger
        .open_peer(0, LedgerId::StubUsd, peer(), 1_100_000_000)
        .unwrap();
    assert!(new);
    let (second, new) = ledger
        .open_peer(1, LedgerId::StubUsd, peer(), 1_100_000_000)
        .unwrap();
    assert!(!new);
    assert_eq!(first, second);
    assert_eq!(net(&ledger, first), 1_100_000_000);
    ledger.check_invariants();
}

#[test]
fn only_what_reaches_posted_balances_is_journaled() {
    let (mut ledger, accounts) = funded(UNIT);
    let mut mirror = Mirror::default();
    mirror.drain(&mut ledger);
    let voided = ledger
        .create(1, &spec(accounts.token, accounts.owner, 3, true, packet(0)))
        .unwrap();
    ledger.void(2, voided).unwrap();
    assert_eq!(ledger.journal_len(), 0, "a voided packet leaves no row");
    let settlement = Reference::Settlement {
        scheme: Name::new("stub").unwrap(),
        id: Name::new("abc").unwrap(),
    };
    let durable = ledger
        .create(
            3,
            &spec(accounts.owner, accounts.settlement, 4, true, settlement),
        )
        .unwrap();
    assert!(matches!(
        ledger.poll_journal(),
        Some(JournalEntry::Transfer {
            state: TransferState::Pending,
            ..
        })
    ));
    ledger.void(4, durable).unwrap();
    assert!(matches!(
        ledger.poll_journal(),
        Some(JournalEntry::Transfer {
            state: TransferState::Voided,
            at_ms: 4,
            ..
        })
    ));
    assert_eq!(ledger.poll_journal(), None);
}

#[test]
fn bounds_refuse_without_changing_anything() {
    let config = Config {
        max_accounts: 8,
        max_pending: 1,
        max_journal: 6,
    };
    let mut ledger = Ledger::new(config);
    let owner = open(&mut ledger, AccountName::EquityOwner);
    let token = open(&mut ledger, AccountName::EquityOwnerSub(OwnerSub::Btp(1)));
    ledger
        .create(0, &spec(token, owner, 1, true, packet(0)))
        .unwrap();
    assert_eq!(
        ledger.create(0, &spec(token, owner, 1, true, packet(1))),
        Err(TransferError::PendingFull)
    );
    // Two accounts opened; four more entries fill the journal.
    for _ in 0..4 {
        ledger
            .create(0, &spec(token, owner, 1, false, packet(2)))
            .unwrap();
    }
    assert_eq!(
        ledger.create(0, &spec(token, owner, 1, false, packet(3))),
        Err(TransferError::JournalFull)
    );
    assert_eq!(ledger.post(0, TransferId(0)), Err(LedgerError::JournalFull));
    assert_eq!(
        ledger.open(path(AccountName::RevenueFx)),
        Err(LedgerError::JournalFull)
    );
    ledger.check_invariants();
    while ledger.poll_journal().is_some() {}
    ledger.post(0, TransferId(0)).unwrap();
    ledger.check_invariants();
}

#[test]
fn totals_stay_within_what_sqlite_stores() {
    let (mut ledger, accounts) = funded(0);
    let max = dassie_ledger::MAX_TOTAL;
    ledger
        .create(
            0,
            &spec(accounts.token, accounts.owner, max, true, packet(0)),
        )
        .unwrap();
    // Pending counts: a later post must never overflow.
    assert_eq!(
        ledger.create(0, &spec(accounts.token, accounts.owner, 1, true, packet(1))),
        Err(TransferError::Overflow)
    );
    ledger.check_invariants();
}

// Reports record reality even past a limit; spending then stops until the
// group recovers.
#[test]
fn reports_may_take_the_group_below_zero() {
    let (mut ledger, accounts) = funded(10);
    ledger
        .create(
            1,
            &spec(
                accounts.owner,
                accounts.suspense,
                15,
                false,
                Reference::Reconciliation,
            ),
        )
        .unwrap();
    let group = ledger.group_balance(accounts.owner).unwrap();
    assert_eq!(group.net_credit(), -5);
    let (peer, _) = ledger.open_peer(0, LedgerId::StubUsd, peer(), 0).unwrap();
    assert_eq!(
        ledger.create(2, &spec(accounts.token, peer, 1, true, packet(0))),
        Err(TransferError::ExceedsDebits)
    );
    ledger
        .create(
            3,
            &spec(
                accounts.settlement,
                accounts.owner,
                6,
                false,
                Reference::Deposit,
            ),
        )
        .unwrap();
    ledger
        .create(4, &spec(accounts.token, peer, 1, true, packet(1)))
        .unwrap();
    ledger.check_invariants();
}

#[test]
fn the_journal_rebuilds_the_ledger() {
    let (mut ledger, accounts) = funded(UNIT);
    let mut mirror = Mirror::default();
    let posted = ledger
        .create(1, &spec(accounts.token, accounts.owner, 9, true, packet(0)))
        .unwrap();
    ledger.post(2, posted).unwrap();
    // In flight at the "crash": gone after the restart.
    ledger
        .create(3, &spec(accounts.token, accounts.owner, 4, true, packet(1)))
        .unwrap();
    let settlement = Reference::Settlement {
        scheme: Name::new("stub").unwrap(),
        id: Name::new("s1").unwrap(),
    };
    let durable = ledger
        .create(
            4,
            &spec(accounts.owner, accounts.settlement, 2, true, settlement),
        )
        .unwrap();
    mirror.drain(&mut ledger);
    let restored = mirror.restore(Config::DEFAULT);
    restored.check_invariants();
    for (id, account) in ledger.accounts() {
        let other = restored.account(id).unwrap();
        assert_eq!(other.path(), account.path());
        assert_eq!(
            other.balance().debits_posted,
            account.balance().debits_posted
        );
        assert_eq!(
            other.balance().credits_posted,
            account.balance().credits_posted
        );
    }
    assert_eq!(
        restored
            .pending_transfers()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        vec![durable]
    );
    assert_eq!(restored.next_transfer_id(), ledger.next_transfer_id());
}

#[test]
fn restore_refuses_damaged_state() {
    let (mut ledger, _) = funded(UNIT);
    let mut mirror = Mirror::default();
    mirror.drain(&mut ledger);
    let restore = |accounts: Vec<dassie_ledger::StoredAccount>| {
        Ledger::restore(Config::DEFAULT, accounts, [], TransferId(100)).map(|_| ())
    };
    let mut unbalanced = mirror.accounts.clone();
    unbalanced[0].debits_posted += 1;
    assert_eq!(
        restore(unbalanced),
        Err(RestoreError::Unbalanced(LedgerId::StubUsd))
    );
    let mut gap = mirror.accounts.clone();
    gap.remove(0);
    assert_eq!(restore(gap), Err(RestoreError::NotDense));
    // The token's sub-account before its group's root.
    let mut orphan = mirror.accounts.clone();
    orphan.swap(1, 2);
    orphan[1].id = dassie_ledger::AccountId(1);
    orphan[2].id = dassie_ledger::AccountId(2);
    assert_eq!(restore(orphan), Err(RestoreError::MissingGroupRoot));
}

// Found by the property test: a pending transfer within the group counted
// as a pending debit of the group, so it used up the pool's free funds and
// broke the group's limit while it was in flight.
#[test]
fn pending_transfers_within_the_group_leave_the_funds_free() {
    let (mut ledger, accounts) = funded(10);
    let (peer, _) = ledger.open_peer(0, LedgerId::StubUsd, peer(), 0).unwrap();
    ledger
        .create(
            1,
            &spec(accounts.token, accounts.owner, 100, true, packet(0)),
        )
        .unwrap();
    let group = ledger.group_balance(accounts.owner).unwrap();
    assert!(group.within(dassie_ledger::Limit::DebitsMustNotExceedCredits));
    ledger
        .create(1, &spec(accounts.token, peer, 10, true, packet(1)))
        .unwrap();
    ledger.check_invariants();
}
