//! Random sequences of operations against the ledger (spec 7 §7 item 2):
//! the invariants hold after every step, no transfer that checks limits
//! breaks one that held, and the journal alone rebuilds the ledger.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

mod support;

use dassie_ledger::{
    AccountId, AccountName, Config, Ledger, LedgerId, Name, OwnerSub, Reference, TraceId,
    TransferSpec,
};
use proptest::prelude::*;
use support::{Mirror, path};

#[derive(Clone, Debug)]
enum Op {
    /// Open the token sub-account `n`.
    OpenToken(u8),
    /// Create a transfer between the `from`-th and `to`-th open accounts.
    Create {
        from: u8,
        to: u8,
        amount: u64,
        pending: bool,
        kind: u8,
    },
    /// Post or void the `n`-th pending transfer.
    Resolve { n: u8, post: bool },
    /// Persist the journal.
    Drain,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        1 => (0u8..6).prop_map(Op::OpenToken),
        6 => (any::<u8>(), any::<u8>(), amount(), any::<bool>(), 0u8..4).prop_map(
            |(from, to, amount, pending, kind)| Op::Create { from, to, amount, pending, kind }
        ),
        3 => (any::<u8>(), any::<bool>()).prop_map(|(n, post)| Op::Resolve { n, post }),
        1 => Just(Op::Drain),
    ]
}

fn amount() -> impl Strategy<Value = u64> {
    prop_oneof![
        0u64..20,
        0u64..1_000_000,
        Just(dassie_ledger::MAX_TOTAL),
        any::<u64>()
    ]
}

fn reference(kind: u8, sequence: u64) -> Reference {
    match kind {
        0 | 1 => Reference::Packet(TraceId { epoch: 0, sequence }),
        2 => Reference::Deposit,
        _ => Reference::Settlement {
            scheme: Name::new("stub").unwrap(),
            id: Name::new(&format!("s{sequence}")).unwrap(),
        },
    }
}

fn pick<T: Copy>(items: &[T], n: u8) -> Option<T> {
    (!items.is_empty()).then(|| items[usize::from(n) % items.len()])
}

/// Whether every limit holds now, per account and per group.
fn limits_held(ledger: &Ledger) -> Vec<bool> {
    let mut held = Vec::new();
    for (id, account) in ledger.accounts() {
        held.push(account.balance().within(account.path().limit()));
        if let Some(group) = ledger.group_balance(id) {
            held.push(group.within(account.path().group_limit()));
        }
    }
    held
}

fn setup() -> (Ledger, Vec<AccountId>) {
    let mut ledger = Ledger::new(Config {
        max_accounts: 32,
        max_pending: 16,
        max_journal: 64,
    });
    let peer = Name::new("peer").unwrap();
    let mut accounts = vec![
        ledger.open(path(AccountName::AssetsSettlement)).unwrap(),
        ledger.open(path(AccountName::EquityOwner)).unwrap(),
        ledger.open(path(AccountName::EquitySuspense)).unwrap(),
    ];
    let (interledger, _) = ledger.open_peer(0, LedgerId::StubUsd, peer, 50).unwrap();
    accounts.push(interledger);
    accounts.push(ledger.find(&path(AccountName::ContraTrust(peer))).unwrap());
    (ledger, accounts)
}

fn run(ops: &[Op]) {
    let (mut ledger, mut accounts) = setup();
    let mut mirror = Mirror::default();
    mirror.drain(&mut ledger);
    for (step, op) in ops.iter().enumerate() {
        let before = limits_held(&ledger);
        let checked = match *op {
            Op::OpenToken(n) => {
                let token = path(AccountName::EquityOwnerSub(OwnerSub::Btp(u64::from(n))));
                if let Ok(id) = ledger.open(token)
                    && !accounts.contains(&id)
                {
                    accounts.push(id);
                }
                true
            }
            Op::Create {
                from,
                to,
                amount,
                pending,
                kind,
            } => {
                let (Some(debit), Some(credit)) = (pick(&accounts, from), pick(&accounts, to))
                else {
                    continue;
                };
                let reference = reference(kind, step as u64);
                let _ = ledger.create(
                    step as u64,
                    &TransferSpec {
                        debit,
                        credit,
                        amount,
                        pending,
                        reference,
                    },
                );
                !reference.is_report()
            }
            Op::Resolve { n, post } => {
                let pending: Vec<_> = ledger.pending_transfers().map(|t| t.id).collect();
                if let Some(id) = pick(&pending, n) {
                    let _ = if post {
                        ledger.post(step as u64, id)
                    } else {
                        ledger.void(step as u64, id)
                    };
                }
                true
            }
            Op::Drain => {
                mirror.drain(&mut ledger);
                true
            }
        };
        ledger.check_invariants();
        if checked {
            // Accounts opened in this step held trivially before it.
            let after = limits_held(&ledger);
            for (index, held) in before.iter().enumerate() {
                assert!(!held || after[index], "step {step} broke a limit: {op:?}");
            }
        }
    }
    mirror.drain(&mut ledger);
    let restored = mirror.restore(ledger.config());
    restored.check_invariants();
    for (id, account) in ledger.accounts() {
        let other = restored.account(id).unwrap();
        assert_eq!(other.path(), account.path());
        let (mine, theirs) = (account.balance(), other.balance());
        assert_eq!(
            theirs.debits_posted,
            mine.debits_posted,
            "{}",
            account.path()
        );
        assert_eq!(
            theirs.credits_posted,
            mine.credits_posted,
            "{}",
            account.path()
        );
    }
    let durable: Vec<_> = ledger
        .pending_transfers()
        .filter(|t| t.reference.is_durable_while_pending())
        .copied()
        .collect();
    let restored_pending: Vec<_> = restored.pending_transfers().copied().collect();
    assert_eq!(restored_pending, durable);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_operations_keep_the_ledger_sound(ops in prop::collection::vec(op(), 0..120)) {
        run(&ops);
    }
}
