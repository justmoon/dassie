//! The account's rules for what it trusts and when it reads, with
//! statements signed by hand; and what the scheme does without a hub.
//! `tests/members.rs` runs both against a real ledger.

use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::NodeKey;
use dassie_ledger::Name;
use dassie_oer::Encode;
use dassie_settlement::{
    AssetEffect, ExternalAsset, LedgerInput, LedgerStatus, LossCause, SchemeError, SettlementScheme,
};

use super::*;
use crate::client::{AccountView, Request, SignedStatement, Statement};
use crate::ledger::Outcome;
use crate::operation::{
    AccountNumber, Destination, MemberOperation, Operation, SignedMemberOperation,
};
use crate::rules::tests::rules;
use crate::validators::ValidatorIndex;

struct Hub {
    rules: crate::rules::Rules,
    validators: Vec<NodeKey>,
    key: Arc<NodeKey>,
}

impl Hub {
    fn new() -> Self {
        let (rules, validators) = rules(4);
        Self {
            rules,
            validators,
            key: Arc::new(NodeKey::from_bytes(&[42; 32])),
        }
    }

    fn account(&self) -> HubAccount {
        HubAccount::new(
            FIRST,
            self.rules.clone(),
            Arc::clone(&self.key),
            Destination {
                account: [5; 20],
                tag: None,
            },
            Name::new(SCHEME_ID).unwrap(),
            &[],
        )
        .unwrap()
    }

    /// Validator `index`'s message of `statements`.
    fn says(&self, index: usize, statements: &[Statement]) -> Vec<u8> {
        let signed: Vec<SignedStatement> = statements
            .iter()
            .map(|s| {
                SignedStatement::sign(
                    &self.rules.id(),
                    ValidatorIndex::new(index).unwrap(),
                    s.clone(),
                    &self.validators[index],
                )
            })
            .collect();
        SignedStatement::encode_list(&signed)
    }

    fn account_statement(&self, height: u64, disbursal: bool) -> Statement {
        Statement::Account {
            height,
            key: self.key.public_key(),
            account: Some((
                AccountNumber(7),
                AccountView {
                    key: self.key.public_key(),
                    balance: 5_000_000,
                    reserve: 2_000_000,
                    sequence: 3,
                    disbursal,
                    closing: false,
                },
            )),
        }
    }

    fn transfer(&self, sequence: u64) -> Statement {
        let signed = SignedMemberOperation::sign(
            &self.rules.id(),
            AccountNumber(9),
            sequence,
            10,
            MemberOperation::Transfer {
                to: AccountNumber(7),
                amount: 5,
            },
            &NodeKey::from_bytes(&[9; 32]),
        );
        Statement::Executed {
            height: 10 + sequence,
            timestamp: 1_000 + sequence,
            index: 0,
            operation: Operation::Member(signed),
            outcome: Outcome::Applied {
                fee: 10,
                detail: crate::ledger::Detail::None,
            },
        }
    }
}

fn from(account: &mut HubAccount, endpoint: usize, message: &[u8]) {
    account.on_ledger(0, LedgerInput::From { endpoint, message });
}

/// The member operations `account` submitted since effects were taken.
fn submitted(account: &mut HubAccount) -> Vec<SignedMemberOperation> {
    let mut effects = Vec::new();
    account.take_effects(&mut effects);
    effects
        .iter()
        .filter_map(|effect| match effect {
            AssetEffect::SendTo { message, .. } => match Request::decode_exact(message) {
                Ok(Request::Submit(Operation::Member(signed))) => Some(signed),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// An account known to validators 0 and 1, as of the block at 5.
fn synced(hub: &Hub) -> HubAccount {
    let mut account = hub.account();
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, true)]),
        );
    }
    account.told();
    account
}

/// Validator `v` says `statements`, a message at most.
fn say_all(hub: &Hub, account: &mut HubAccount, v: usize, statements: &[Statement]) {
    for chunk in statements.chunks(crate::client::MAX_STATEMENTS) {
        from(account, v, &hub.says(v, chunk));
    }
}

fn read_count(account: &HubAccount) -> usize {
    account
        .news()
        .iter()
        .filter(|n| matches!(n, News::Executed { .. }))
        .count()
}

// One faulty validator signing junk between the honest answers never makes
// the account pass an operation it did not read.
#[test]
fn a_flood_from_one_validator_never_hides_an_operation() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let answer = [
        hub.transfer(1),
        Statement::Final {
            height: 20,
            timestamp: 2_000,
        },
    ];
    let junk: Vec<Statement> = (0..2_000u64)
        .map(|k| Statement::Final {
            height: 21 + k,
            timestamp: k,
        })
        .collect();
    for v in 0..3 {
        say_all(&hub, &mut account, v, &answer);
        say_all(&hub, &mut account, 3, &junk);
        account.check_invariants();
    }
    assert_eq!(account.cursor(), 20);
    assert_eq!(read_count(&account), 1, "the operation was read");
}

// When the account drops what it agreed above its cursor, no `Final` that
// was on its way counts: the validators answer anew first.
#[test]
fn a_full_buffer_moves_no_coverage_past_what_it_dropped() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let bound = super::account::max_buffered(&hub.rules.terms);
    let many: Vec<Statement> = (1..=u64::try_from(bound).unwrap() + 1)
        .map(|sequence| hub.transfer(sequence))
        .collect();
    for v in 0..2 {
        say_all(&hub, &mut account, v, &many);
    }
    let final_at = |height: u64| Statement::Final {
        height,
        timestamp: 1_000 + height,
    };
    for v in 0..3 {
        say_all(&hub, &mut account, v, &[final_at(5_000)]);
    }
    assert_eq!(account.cursor(), 5, "nothing it dropped is passed");
    // The validators answer anew.
    for v in 0..3 {
        say_all(&hub, &mut account, v, &[hub.transfer(1), final_at(11)]);
    }
    assert_eq!(account.cursor(), 11);
    assert_eq!(read_count(&account), 1);
    account.check_invariants();
}

// A faulty leader can put our operation in a block twice: applied, then
// refused for its sequence. The account reads both, in the log's order,
// and takes the first for what became of it.
#[test]
fn an_operation_twice_in_a_block_is_read_as_applied() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let settlement = Name::new("s").unwrap();
    let fund = MemberOperation::HoldFund {
        hold: crate::promise::HoldNumber(1),
        amount: 1,
    };
    account.prepare(settlement, fund).unwrap();
    assert!(account.execute(settlement));
    let sent = submitted(&mut account);
    let ours = sent[0].clone();
    let copy = |index: u32, outcome: Outcome| Statement::Executed {
        height: 10,
        timestamp: 1_000,
        index,
        operation: Operation::Member(ours.clone()),
        outcome,
    };
    let applied = Outcome::Applied {
        fee: 10,
        detail: crate::ledger::Detail::None,
    };
    let refused = Outcome::Refused(crate::ledger::Refusal::Sequence);
    let final_block = Statement::Final {
        height: 20,
        timestamp: 2_000,
    };
    for v in 0..3 {
        let answer = [copy(0, applied), copy(1, refused), final_block.clone()];
        from(&mut account, v, &hub.says(v, &answer));
    }
    assert_eq!(account.cursor(), 20);
    let read: Vec<Option<Purpose>> = account
        .news()
        .iter()
        .filter_map(|news| match news {
            News::Executed { ours, .. } => Some(*ours),
            _ => None,
        })
        .collect();
    assert_eq!(read, [Some(Purpose::Settle(settlement))], "read as applied");
    assert!(!account.has(Purpose::Settle(settlement)));
    account.on_timer(10_000);
    assert!(submitted(&mut account).is_empty(), "never signed again");
    account.check_invariants();
}

#[test]
fn amounts_convert_exactly() {
    assert_eq!(units(3), 3_000);
    assert_eq!(
        units(Amount::MAX),
        MAX_DROPS * UNITS_PER_DROP,
        "held at the most"
    );
    assert_eq!(drops_covering(0), 0);
    assert_eq!(drops_covering(1), 1);
    assert_eq!(drops_covering(1_000), 1);
    assert_eq!(drops_covering(1_001), 2);
}

// spec/areas/hubs.md#the-members-side: a member trusts what f + 1
// validators signed alike.
#[test]
fn the_account_is_known_once_f_plus_one_agree() {
    let hub = Hub::new();
    let mut account = hub.account();
    assert_eq!(account.status(), LedgerStatus::Connecting);
    account.on_timer(0);
    let mut asked = Vec::new();
    account.take_effects(&mut asked);
    let ask = Request::Account(hub.key.public_key()).to_vec();
    assert_eq!(
        asked,
        (0..4)
            .map(|endpoint| AssetEffect::SendTo {
                endpoint,
                message: ask.clone()
            })
            .collect::<Vec<_>>()
    );
    from(
        &mut account,
        0,
        &hub.says(0, &[hub.account_statement(5, true)]),
    );
    assert_eq!(account.number(), None, "one validator is not enough");
    // A validator saying it twice counts once.
    from(
        &mut account,
        1,
        &hub.says(0, &[hub.account_statement(5, true)]),
    );
    assert_eq!(account.number(), None);
    from(
        &mut account,
        1,
        &hub.says(1, &[hub.account_statement(5, true)]),
    );
    assert_eq!(account.number(), Some(AccountNumber(7)));
    assert!(account.is_synced());
    assert_eq!(account.status(), LedgerStatus::Synced);
    assert!(matches!(account.news(), [News::Account { height: 5, .. }]));
    account.check_invariants();
}

// An operation is read only once a quorum covered its block, each from
// its own endpoint: a validator's word on what it covered, passed on by
// another, counts for nothing.
#[test]
fn operations_are_read_once_a_quorum_covered_them() {
    let hub = Hub::new();
    let mut account = hub.account();
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, true)]),
        );
    }
    account.told();
    let executed = hub.transfer(1);
    let final_block = Statement::Final {
        height: 20,
        timestamp: 2_000,
    };
    let answer = |v: usize| hub.says(v, &[executed.clone(), final_block.clone()]);
    from(&mut account, 0, &answer(0));
    from(&mut account, 1, &answer(1));
    assert_eq!(account.cursor(), 5, "agreed, but two validators covered it");
    // Validator 2's answer, from validator 3's endpoint, covers nothing.
    from(&mut account, 3, &answer(2));
    assert_eq!(account.cursor(), 5);
    from(&mut account, 2, &answer(2));
    assert_eq!(account.cursor(), 20);
    let read: Vec<&News> = account
        .news()
        .iter()
        .filter(|n| matches!(n, News::Executed { .. }))
        .collect();
    assert_eq!(read.len(), 1, "read once");
    assert!(matches!(
        account.news().last(),
        Some(News::Final { height: 20, .. })
    ));
    // Said again, it is not read again.
    account.told();
    from(&mut account, 3, &answer(3));
    assert!(account.news().is_empty());
    account.check_invariants();
}

// spec/designs/settlement-hubs.md#5-joining: a new account sets its
// disbursal address before anything else.
#[test]
fn a_new_account_sets_its_disbursal_address_first() {
    let hub = Hub::new();
    let mut account = hub.account();
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, false)]),
        );
    }
    let mut asked = Vec::new();
    account.take_effects(&mut asked);
    let submitted: Vec<MemberOperation> = asked
        .iter()
        .filter_map(|effect| match effect {
            AssetEffect::SendTo { message, .. } => match Request::decode_exact(message) {
                Ok(Request::Submit(Operation::Member(signed))) => Some(signed.operation),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(submitted.len(), 4, "to every validator");
    assert!(matches!(submitted[0], MemberOperation::SetDisbursal(_)));
    // Nothing else goes before it has.
    account
        .submit(
            Purpose::Redeem(crate::promise::HoldNumber(1)),
            MemberOperation::HoldRelease {
                hold: crate::promise::HoldNumber(1),
            },
        )
        .unwrap();
    let mut asked = Vec::new();
    account.take_effects(&mut asked);
    assert!(asked.is_empty(), "one operation on its way at a time");
    account.check_invariants();
}

#[test]
fn the_queue_is_bounded_and_waits_for_the_account() {
    let hub = Hub::new();
    let mut account = hub.account();
    let fund = |n: u64| MemberOperation::HoldFund {
        hold: crate::promise::HoldNumber(n),
        amount: 1,
    };
    assert_eq!(
        account.prepare(Name::new("s0").unwrap(), fund(0)),
        Err(SchemeError::Unavailable),
        "not before it knows its account"
    );
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, true)]),
        );
    }
    for n in 0..account::MAX_QUEUED {
        let id = Name::new(&alloc::format!("s{n}")).unwrap();
        account
            .prepare(id, fund(u64::try_from(n).unwrap()))
            .unwrap();
    }
    assert_eq!(
        account.prepare(Name::new("full").unwrap(), fund(99)),
        Err(SchemeError::Unavailable)
    );
    // Prepared is not sent: only once executed.
    let mut asked = Vec::new();
    account.take_effects(&mut asked);
    assert!(asked.is_empty());
    assert!(account.execute(Name::new("s0").unwrap()));
    account.take_effects(&mut asked);
    assert_eq!(asked.len(), 4);
    account.discard(Name::new("s1").unwrap());
    assert!(!account.has(Purpose::Settle(Name::new("s1").unwrap())));
    account.check_invariants();
}

#[test]
fn the_scheme_books_nothing_on_its_own() {
    let hub = Hub::new();
    let account = hub.account();
    let mut scheme = HubScheme::new(&account, 100_000_000, None, &[]).unwrap();
    let mut effects = Vec::new();
    assert_eq!(
        scheme.deposit(5, &mut effects),
        Err(SchemeError::Unsupported)
    );
    let peer = Name::new("peer").unwrap();
    assert_eq!(
        scheme.handle_settlement(peer, 5, &[0; 16], &[], &mut effects),
        Ok(())
    );
    assert_eq!(
        scheme.handle_settlement(peer, 5, &[0; 3], &[], &mut effects),
        Err(SchemeError::InvalidData)
    );
    assert!(effects.is_empty());
    assert_eq!(scheme.unit(), UNITS_PER_DROP);
    assert!(!scheme.has_synced(), "not before it knows its account");
    let mut info = Vec::new();
    scheme.peering_info(&mut info);
    assert!(info.is_empty());
    // A scheme not in step with its account is refused.
    let mut other = hub.account();
    for v in 0..2 {
        from(
            &mut other,
            v,
            &hub.says(v, &[hub.account_statement(5, true)]),
        );
    }
    let state = scheme.state();
    assert_eq!(
        HubScheme::new(&other, 100_000_000, None, &state).err(),
        Some(SchemeError::InvalidData)
    );
    scheme.check_invariants();
}

#[test]
fn losses_of_each_cause_are_booked_once_each() {
    let operation = crate::digest::Digest([7; 32]);
    let ids: alloc::collections::BTreeSet<Name> =
        [LossCause::ClaimFee, LossCause::PaidLate, LossCause::Cheated]
            .into_iter()
            .map(|cause| scheme::loss_id(&operation, cause))
            .collect();
    assert_eq!(ids.len(), 3);
}
