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
use crate::service::wire::Message;
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
        Message::Statements(signed).to_vec()
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
                    deposited: 7_000_000,
                    withdrawn: 0,
                    returned: 0,
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
            AssetEffect::SendTo { message, .. } => match Message::decode_exact(message) {
                Ok(Message::Request(Request::Submit(Operation::Member(signed)))) => Some(signed),
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

// The holds read anew come over several pages: the account takes them only
// once every page is in, so it never takes part of the list for the whole.
#[test]
fn holds_read_anew_wait_for_every_page() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let trimmed = Statement::Trimmed {
        height: 100,
        below: 50,
    };
    let view = crate::client::HoldView {
        payer: AccountNumber(7),
        payee: AccountNumber(9),
        amount: 10,
        reduction: None,
        total: 0,
        through: 0,
    };
    let page = |page: u16| Statement::Holds {
        height: 100,
        timestamp: 9_000,
        account: AccountNumber(7),
        page,
        pages: 2,
        holds: alloc::vec![(crate::promise::HoldNumber(u64::from(page) + 1), view)],
    };
    for v in 0..2 {
        say_all(&hub, &mut account, v, core::slice::from_ref(&trimmed));
    }
    assert!(account.is_resyncing());
    for v in 0..2 {
        say_all(
            &hub,
            &mut account,
            v,
            &[hub.account_statement(100, true), page(0)],
        );
    }
    assert!(account.is_resyncing(), "one page of two");
    for v in 0..2 {
        say_all(&hub, &mut account, v, &[page(1)]);
    }
    assert!(!account.is_resyncing());
    assert_eq!(account.holds().count(), 2);
    assert_eq!(account.cursor(), 100);
    account.check_invariants();
}

// spec/areas/hubs.md#when-the-history-is-gone: with one validator silent
// (f) and one honest one restarted past the cursor, too few cover it for
// the cursor to move, and one `Trimmed` is too few for f + 1 to agree.
// Once the cursor has not moved for `STUCK_MS` since, the account reads its
// state anew, taking only what f + 1 sign alike, and goes on from there.
#[test]
fn an_account_stuck_behind_a_trimmed_history_reads_anew() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let fin = Statement::Final {
        height: 100,
        timestamp: 9_000,
    };
    let trimmed = Statement::Trimmed {
        height: 100,
        below: 50,
    };
    for v in 0..2 {
        say_all(&hub, &mut account, v, core::slice::from_ref(&fin));
    }
    say_all(&hub, &mut account, 2, core::slice::from_ref(&trimmed));
    assert_eq!(account.cursor(), 5, "two of four cover 100");
    account.on_timer(account::STUCK_MS - 1);
    assert!(!account.is_resyncing(), "not stuck yet");
    account.on_timer(account::STUCK_MS);
    assert!(account.is_resyncing());
    let holds = Statement::Holds {
        height: 100,
        timestamp: 9_000,
        account: AccountNumber(7),
        page: 0,
        pages: 1,
        holds: Vec::new(),
    };
    for v in 0..2 {
        say_all(
            &hub,
            &mut account,
            v,
            &[hub.account_statement(100, true), holds.clone()],
        );
    }
    assert!(!account.is_resyncing());
    assert_eq!(account.cursor(), 100);
    account.check_invariants();
}

// One validator that no longer keeps the history since the cursor, and
// another cut off for a while, as in a rolling restart during a network
// blip: too few cover the cursor until the second is back, well within
// `STUCK_MS`, and the account reads in order, never anew, which would
// skip the blocks in between and lose their claims' fulfillments.
#[test]
fn one_trimmed_and_a_short_outage_read_in_order() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let trimmed = Statement::Trimmed {
        height: 100,
        below: 50,
    };
    let fin = Statement::Final {
        height: 100,
        timestamp: 9_000,
    };
    say_all(&hub, &mut account, 2, core::slice::from_ref(&trimmed));
    for v in [0, 1] {
        say_all(&hub, &mut account, v, core::slice::from_ref(&fin));
    }
    // Validator 3 is cut off for a minute.
    for at in (1_000..=60_000).step_by(1_000) {
        account.on_timer(at);
        assert!(!account.is_resyncing(), "anew at {at}");
    }
    assert_eq!(account.cursor(), 5, "two of four cover 100");
    from(&mut account, 3, &hub.says(3, core::slice::from_ref(&fin)));
    assert_eq!(account.cursor(), 100, "read in order");
    account.on_timer(account::STUCK_MS * 2);
    assert!(!account.is_resyncing());
    account.check_invariants();
}

// A `Trimmed` heard before the account is known starts nothing: the
// cursor is set from the account's state once it is.
#[test]
fn a_trimmed_before_the_account_is_known_starts_nothing() {
    let hub = Hub::new();
    let mut account = hub.account();
    let trimmed = Statement::Trimmed {
        height: 100,
        below: 50,
    };
    from(
        &mut account,
        2,
        &hub.says(2, core::slice::from_ref(&trimmed)),
    );
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, true)]),
        );
    }
    account.on_timer(account::STUCK_MS);
    assert!(!account.is_resyncing());
    account.check_invariants();
}

// A validator that no longer keeps the history since the cursor is no
// reason to read anew while a quorum still moves the cursor.
#[test]
fn a_trimmed_history_a_quorum_covers_is_read_in_order() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let trimmed = Statement::Trimmed {
        height: 100,
        below: 50,
    };
    say_all(&hub, &mut account, 2, core::slice::from_ref(&trimmed));
    for v in [0, 1, 3] {
        let fin = Statement::Final {
            height: 100,
            timestamp: 9_000,
        };
        say_all(&hub, &mut account, v, &[fin]);
    }
    assert_eq!(account.cursor(), 100);
    account.on_timer(account::STUCK_MS * 2);
    assert!(!account.is_resyncing());
    account.check_invariants();
}

/// A hold `account` opened toward account 7 at `height`, as hold `number`.
fn opened_toward_us(hub: &Hub, account: u32, number: u64, height: u64) -> Statement {
    let signed = SignedMemberOperation::sign(
        &hub.rules.id(),
        AccountNumber(account),
        0,
        10,
        MemberOperation::HoldOpen {
            payee: AccountNumber(7),
            amount: 1,
        },
        &NodeKey::from_bytes(&[9; 32]),
    );
    Statement::Executed {
        height,
        timestamp: height,
        index: 0,
        operation: Operation::Member(signed),
        outcome: Outcome::Applied {
            fee: 10,
            detail: crate::ledger::Detail::Hold(crate::promise::HoldNumber(number)),
        },
    }
}

// Anyone may open holds toward the account: those of accounts the scheme
// does not follow wait in a pool of their own, and never take the place of
// its peers' holds.
#[test]
fn holds_strangers_open_toward_us_take_no_peers_place() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let mut statements: Vec<Statement> = (0..100u32)
        .map(|i| opened_toward_us(&hub, 100 + i, u64::from(i) + 1, 10 + u64::from(i)))
        .collect();
    // The peer's hold, opened before its peering was made.
    statements.push(opened_toward_us(&hub, 9, 500, 200));
    statements.push(Statement::Final {
        height: 300,
        timestamp: 300,
    });
    for v in 0..3 {
        say_all(&hub, &mut account, v, &statements);
    }
    assert_eq!(account.cursor(), 300);
    assert!(account.strangers.len() <= account::MAX_STRANGER_HOLDS);
    assert!(account.hold(crate::promise::HoldNumber(500)).is_some());
    account.follow([AccountNumber(9)]);
    assert!(!account.strangers.contains(&crate::promise::HoldNumber(500)));
    // Strangers keep coming; the peer's hold stays.
    let more: Vec<Statement> = (0..100u32)
        .map(|i| opened_toward_us(&hub, 300 + i, 1_000 + u64::from(i), 301 + u64::from(i)))
        .chain([Statement::Final {
            height: 500,
            timestamp: 500,
        }])
        .collect();
    for v in 0..3 {
        say_all(&hub, &mut account, v, &more);
    }
    assert_eq!(account.cursor(), 500);
    assert!(account.hold(crate::promise::HoldNumber(500)).is_some());
    assert_eq!(account.holds().count(), account::MAX_STRANGER_HOLDS + 1);
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
    let ask = Message::Request(Request::Account(hub.key.public_key())).to_vec();
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
            AssetEffect::SendTo { message, .. } => match Message::decode_exact(message) {
                Ok(Message::Request(Request::Submit(Operation::Member(signed)))) => {
                    Some(signed.operation)
                }
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
    for n in 0..account::MAX_QUEUED_WITHOUT_DEADLINE {
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

// A peer that comes back with another key, with nothing left to settle,
// starts afresh: the hub has not confirmed the new key.
#[test]
fn a_new_key_is_confirmed_anew() {
    let hub = Hub::new();
    let account = hub.account();
    let mut scheme = HubScheme::new(&account, 100_000_000, None, &[]).unwrap();
    let peer = Name::new("peer").unwrap();
    let state = |key: &NodeKey| {
        let mut out = Vec::new();
        peering::Membership {
            hub: hub.rules.id(),
            account: AccountNumber(9),
            key: key.public_key(),
        }
        .write(&mut out);
        out
    };
    scheme.on_peered(peer, &state(&NodeKey::from_bytes(&[9; 32])));
    scheme.peers.get_mut(&peer).unwrap().payee.key_confirmed = true;
    let other = state(&NodeKey::from_bytes(&[10; 32]));
    scheme.accept_peering_request(peer, &other).unwrap();
    scheme.on_peered(peer, &other);
    let known = &scheme.peers[&peer];
    assert_eq!(known.key, NodeKey::from_bytes(&[10; 32]).public_key());
    assert!(!known.payee.key_confirmed);
    scheme.check_invariants();
}

// spec/decisions/closing-channels.md: a hold with funds keeps a former
// peer's settlement channel open; once nothing is left to settle, the
// channel may close, and its records go with it.
#[test]
fn a_hold_with_funds_keeps_the_channel() {
    let hub = Hub::new();
    let account = hub.account();
    let mut scheme = HubScheme::new(&account, 100_000_000, None, &[]).unwrap();
    let peer = Name::new("peer").unwrap();
    let mut state = Vec::new();
    peering::Membership {
        hub: hub.rules.id(),
        account: AccountNumber(9),
        key: NodeKey::from_bytes(&[9; 32]).public_key(),
    }
    .write(&mut state);
    scheme.on_peered(peer, &state);
    assert!(!scheme.keeps_channel(peer));
    let known = scheme.peers.get_mut(&peer).unwrap();
    known.payee.hold = Some(crate::promise::HoldNumber(1));
    known.payee.snapshot.amount = 5;
    assert!(scheme.keeps_channel(peer), "funds in its hold toward us");
    let known = scheme.peers.get_mut(&peer).unwrap();
    known.payee.snapshot.amount = 0;
    assert!(!scheme.keeps_channel(peer));
    scheme.on_closed(peer);
    assert!(!scheme.peers.contains_key(&peer));
    scheme.check_invariants();
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
            .map(|cause| {
                scheme::loss_id(&payer::Loss {
                    units: 1,
                    cause,
                    operation,
                    hold: crate::promise::HoldNumber(1),
                    sequence: 1,
                })
            })
            .collect();
    assert_eq!(ids.len(), 3);
}

// An account restored with its operation on its way sends it again once it
// is in step with the hub; until then nothing is due for it, so a host
// does not wake it over and over at one instant.
#[test]
fn an_operation_on_its_way_waits_for_the_restored_account_to_sync() {
    let hub = Hub::new();
    let mut account = hub.account();
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, false)]),
        );
    }
    assert_eq!(submitted(&mut account).len(), 4, "the disbursal address");
    let state = account.state();
    let mut restored = HubAccount::new(
        FIRST,
        hub.rules.clone(),
        Arc::clone(&hub.key),
        Destination {
            account: [5; 20],
            tag: None,
        },
        Name::new(SCHEME_ID).unwrap(),
        &state,
    )
    .unwrap();
    assert!(!restored.is_synced());
    let now = 1_800_000_000_000;
    restored.on_timer(now);
    assert!(
        restored.deadline().is_none_or(|at| at > now),
        "due again at once: {:?}",
        restored.deadline()
    );
    // In step with the hub again, it sends the operation at once.
    let _ = submitted(&mut restored);
    let caught_up = [
        hub.account_statement(6, false),
        Statement::Final {
            height: 6,
            timestamp: 1_006,
        },
    ];
    for v in 0..3 {
        restored.on_ledger(
            now,
            LedgerInput::From {
                endpoint: v,
                message: &hub.says(v, &caught_up),
            },
        );
    }
    assert!(restored.is_synced());
    restored.on_timer(now);
    let again = submitted(&mut restored);
    assert!(
        again
            .iter()
            .any(|signed| matches!(signed.operation, MemberOperation::SetDisbursal(_))),
        "{again:?}"
    );
    restored.check_invariants();
}

// A deposit reaches the account's state before the scheme is told of it:
// the attestations that credit it are no operation the account reads, so
// the scheme books it with the state that follows. Until then, a
// settlement spends only what the books hold of the balance: spent before
// it was booked, it would take the books below nothing.
#[test]
fn a_settlement_spends_only_what_the_books_hold() {
    let hub = Hub::new();
    let mut account = hub.account();
    let mut scheme = HubScheme::new(&account, 100_000_000, None, &[]).unwrap();
    let caught_up = Statement::Final {
        height: 5,
        timestamp: 1_005,
    };
    for v in 0..3 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, true), caught_up.clone()]),
        );
    }
    let mut effects = Vec::new();
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    assert_eq!(scheme.balance(), units(5_000_000), "the first state booked");
    // The hub credits a deposit of 10 XRP; f + 1 validators say so before
    // a quorum covered the block that did.
    let mut newer = hub.account_statement(6, true);
    if let Statement::Account {
        account: Some((_, view)),
        ..
    } = &mut newer
    {
        view.balance += 10_000_000;
        view.deposited += 10_000_000;
    }
    // A transfer toward us at 11 is read in order; the state at 6 is older
    // than what touched the account since, so the scheme is not told it.
    let read = [
        newer,
        hub.transfer(1),
        Statement::Final {
            height: 11,
            timestamp: 1_011,
        },
    ];
    for v in 0..3 {
        from(&mut account, v, &hub.says(v, &read));
    }
    assert_eq!(account.view().map(|(h, _)| *h), Some(6));
    assert!(account.is_synced());
    let mut effects = Vec::new();
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    assert_eq!(scheme.balance(), units(5_000_000), "not told of it yet");
    let peer = Name::new("peer").unwrap();
    let mut state = Vec::new();
    super::peering::Membership {
        hub: hub.rules.id(),
        account: AccountNumber(9),
        key: NodeKey::from_bytes(&[9; 32]).public_key(),
    }
    .write(&mut state);
    let mut rng = dassie_kit::sim::SeededRng::new(1);
    let prepared = scheme
        .prepare_settlement(
            &mut rng,
            Some(&mut account),
            peer,
            units(12_000_000),
            &state,
        )
        .unwrap();
    assert!(
        prepared.amount <= units(5_000_000),
        "spent {} of the books' {}",
        prepared.amount,
        units(5_000_000)
    );
    scheme.check_invariants();
}

// The default testnet hub built in, once one is, is the one founded on the
// testnet: its rules decode, check, with a margin that covers finality
// with one of its validators faulty, and have boot1–4 as its validators,
// so a release can never ship another by mistake
// (spec/designs/settlement-hubs.md#11-choosing-a-hub).
#[test]
fn the_default_testnet_hub_is_the_one_founded() {
    let Some(hex) = DEFAULT_TESTNET_HUB else {
        return;
    };
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let mut reader = dassie_oer::Reader::new(&bytes);
    let rules = crate::rules::Rules::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_eq!(rules.check(), Ok(()));
    assert!(
        rules.terms.timers.margin >= 600,
        "M covers one faulty validator of four"
    );
    assert_eq!(rules.terms.network, 1, "the XRP Ledger testnet");
    // Each validator is the bootstrap node it names, by its node key.
    let nodes: Vec<alloc::string::String> = rules
        .validators
        .iter()
        .map(|acceptance| acceptance.node.node_id().as_str().into())
        .collect();
    assert_eq!(
        nodes,
        [
            "br5l3hqpjhlt5znzb24hrxqme77d7q7crlqfhza",
            "cnayj52pxvacichpys3p4hsqnbxtliup3rkkz5i",
            "wu3lfwpoqblye7pdjhcspl6whzcux6j4vnkkdfq",
            "ywpqifdgvbalxycja2oiu4lg5ogogwpagqdtk7q"
        ]
    );
    let endpoints: Vec<&str> = rules
        .validators
        .iter()
        .map(|acceptance| acceptance.endpoint.as_str())
        .collect();
    assert_eq!(
        endpoints,
        [
            "boot1.dassie.xyz",
            "boot2.dassie.xyz",
            "boot3.dassie.xyz",
            "boot4.dassie.xyz"
        ]
    );
}

/// The hub's word that it has no account for the member's key, as of
/// `height`.
fn none_statement(hub: &Hub, height: u64) -> Statement {
    Statement::Account {
        height,
        key: hub.key.public_key(),
        account: None,
    }
}

/// The member's account 7 as of `height`, opened by a `pay` of `paid`:
/// nothing of it deposited.
fn paid_in_statement(hub: &Hub, height: u64, paid: u64) -> Statement {
    let reserve = hub.rules.terms.fees.account_reserve;
    Statement::Account {
        height,
        key: hub.key.public_key(),
        account: Some((
            AccountNumber(7),
            AccountView {
                key: hub.key.public_key(),
                balance: paid - reserve,
                reserve,
                sequence: 0,
                disbursal: false,
                closing: false,
                deposited: 0,
                withdrawn: 0,
                returned: 0,
            },
        )),
    }
}

/// The payer's key: account 9's.
fn payer_key() -> NodeKey {
    NodeKey::from_bytes(&[9; 32])
}

/// Account 9's `pay` of `paid` to the member's key, which opened its
/// account 7 in the block at `height`.
fn pay_statement(hub: &Hub, height: u64, paid: u64) -> Statement {
    let signed = SignedMemberOperation::sign(
        &hub.rules.id(),
        AccountNumber(9),
        0,
        10,
        MemberOperation::Pay {
            key: hub.key.public_key(),
            amount: paid,
        },
        &payer_key(),
    );
    Statement::Executed {
        height,
        timestamp: 1_000 + height,
        index: 0,
        operation: Operation::Member(signed),
        outcome: Outcome::Applied {
            fee: 10,
            detail: crate::ledger::Detail::Account(AccountNumber(7)),
        },
    }
}

/// What `paid_in` pays the member: the hub's minimum for a new account.
fn minimum(hub: &Hub) -> u64 {
    hub.rules.terms.new_account_minimum()
}

// spec/designs/settlement-hubs.md#5-joining: an account the hub had none
// for reads what opened it in order, from where f + 1 validators agreed it
// had none, a peer's `pay` among it; then sets its disbursal address before
// anything else.
#[test]
fn an_account_opened_since_reads_what_opened_it() {
    let hub = Hub::new();
    let mut account = hub.account();
    for v in 0..2 {
        from(&mut account, v, &hub.says(v, &[none_statement(&hub, 5)]));
    }
    assert_eq!(account.status(), LedgerStatus::Unfunded);
    assert_eq!(account.cursor(), 5, "nothing touched what did not exist");
    assert!(matches!(account.news(), [News::Unfunded]));
    account.told();
    let paid = minimum(&hub);
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[paid_in_statement(&hub, 9, paid)]),
        );
    }
    assert_eq!(account.number(), Some(AccountNumber(7)));
    assert_eq!(account.cursor(), 5, "read from where it had none");
    assert!(!account.is_synced());
    assert!(
        account.news().is_empty(),
        "its state waits for what led there"
    );
    assert!(submitted(&mut account).is_empty());
    let final_block = Statement::Final {
        height: 9,
        timestamp: 1_009,
    };
    for v in 0..3 {
        let answer = [pay_statement(&hub, 7, paid), final_block.clone()];
        from(&mut account, v, &hub.says(v, &answer));
    }
    assert_eq!(account.cursor(), 9);
    assert!(account.is_synced());
    assert!(
        matches!(
            account.news(),
            [
                News::Executed {
                    height: 7,
                    ours: None,
                    ..
                },
                News::Final { height: 9, .. },
                News::Account { height: 9, .. }
            ]
        ),
        "{:?}",
        account.news()
    );
    let first = submitted(&mut account);
    assert!(matches!(
        first[0].operation,
        MemberOperation::SetDisbursal(_)
    ));
    assert_eq!(first[0].sequence, 0);
    account.check_invariants();
}

/// A scheme told by f + 1 validators that the hub has no account for its
/// key, as of the block at 5.
fn without_an_account(hub: &Hub) -> (HubAccount, HubScheme) {
    let mut account = hub.account();
    let mut scheme = HubScheme::new(&account, 100_000_000, None, &[]).unwrap();
    assert_eq!(scheme.commitments().unwrap().membership(), None);
    for v in 0..2 {
        from(&mut account, v, &hub.says(v, &[none_statement(hub, 5)]));
    }
    let mut effects = Vec::new();
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    assert!(effects.is_empty());
    (account, scheme)
}

/// The peer of account 9, as its peering info presents it.
fn payer_state(hub: &Hub) -> Vec<u8> {
    let mut state = Vec::new();
    peering::Membership {
        hub: hub.rules.id(),
        account: AccountNumber(9),
        key: payer_key().public_key(),
    }
    .write(&mut state);
    state
}

// spec/designs/settlement-hubs.md#5-joining: a scheme whose key the hub has
// no account for runs, presents its key with no account, peers only with a
// member that has one, and takes no promises from it.
#[test]
fn a_scheme_without_an_account_runs() {
    let hub = Hub::new();
    let (_, mut scheme) = without_an_account(&hub);
    assert!(scheme.has_synced(), "it runs without a deposit");
    assert_eq!(scheme.balance(), 0);
    let mut info = Vec::new();
    scheme.peering_info(&mut info);
    let ours = peering::Membership::read(&info, &hub.rules.id()).unwrap();
    assert_eq!(ours.account, peering::NONE);
    assert_eq!(ours.key, hub.key.public_key());
    // Not advertised until the node carries packets without promises
    // (spec/roadmap.md#joining-by-being-paid).
    assert_eq!(scheme.commitments().unwrap().membership(), None);
    // Two members without an account do not peer.
    let mut keyless = Vec::new();
    peering::Membership {
        hub: hub.rules.id(),
        account: peering::NONE,
        key: NodeKey::from_bytes(&[8; 32]).public_key(),
    }
    .write(&mut keyless);
    let other = Name::new("other").unwrap();
    assert_eq!(
        scheme.accept_peering_request(other, &keyless),
        Err(SchemeError::Unavailable)
    );
    let mut out = Vec::new();
    assert_eq!(
        scheme.create_peering_request(other, &keyless, &mut out),
        Err(SchemeError::Unavailable)
    );
    // One with an account peers, and promises nothing either way.
    let payer = Name::new("payer").unwrap();
    let state = payer_state(&hub);
    assert_eq!(
        scheme.accept_peering_request(payer, &state),
        Ok(state.clone())
    );
    scheme.on_peered(payer, &state);
    let commitments = scheme.commitments().unwrap();
    assert!(!commitments.commits_with(payer));
    assert_eq!(commitments.opening_minimum(payer), None, "we pay no one in");
    scheme.check_invariants();
}

// spec/designs/settlement-hubs.md#5-joining: the `pay` that opened the
// account is the paying peer's settlement, read once, and the account
// reserve is the reserve's; from then on the scheme is a member as any.
#[test]
fn a_scheme_paid_in_books_its_peers_settlement() {
    let hub = Hub::new();
    let (mut account, mut scheme) = without_an_account(&hub);
    let payer = Name::new("payer").unwrap();
    scheme.on_peered(payer, &payer_state(&hub));
    let paid = minimum(&hub);
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[paid_in_statement(&hub, 9, paid)]),
        );
    }
    let final_block = Statement::Final {
        height: 9,
        timestamp: 1_009,
    };
    for v in 0..3 {
        let answer = [pay_statement(&hub, 7, paid), final_block.clone()];
        from(&mut account, v, &hub.says(v, &answer));
    }
    let mut effects = Vec::new();
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    let reserve = hub.rules.terms.fees.account_reserve;
    assert_eq!(
        effects,
        [
            dassie_settlement::Effect::IncomingSettlement {
                peer: payer,
                amount: units(paid),
                id: effects.first().and_then(|effect| match effect {
                    dassie_settlement::Effect::IncomingSettlement { id, .. } => *id,
                    _ => None,
                }),
            },
            dassie_settlement::Effect::Reconcile {
                amount: units(reserve),
                gain: false,
                cause: dassie_settlement::ReconcileCause::Reserve,
            },
        ]
    );
    assert_eq!(scheme.balance(), units(paid - reserve));
    assert!(scheme.commitments().unwrap().commits_with(payer));
    assert_eq!(
        scheme.commitments().unwrap().membership().unwrap().account,
        7
    );
    let mut info = Vec::new();
    scheme.peering_info(&mut info);
    assert_eq!(
        peering::Membership::read(&info, &hub.rules.id())
            .unwrap()
            .account,
        AccountNumber(7)
    );
    // Read once: said again, nothing more is booked.
    for v in 0..4 {
        let answer = [pay_statement(&hub, 7, paid), final_block.clone()];
        from(&mut account, v, &hub.says(v, &answer));
    }
    let mut effects = Vec::new();
    scheme.on_asset(0, &mut account, &mut effects);
    assert!(effects.is_empty());
    scheme.check_invariants();
    account.check_invariants();
}

/// A scheme on an account known to validators 0 and 1, as of the block at
/// 5.
fn synced_scheme(hub: &Hub) -> (HubAccount, HubScheme) {
    let mut account = hub.account();
    let mut scheme = HubScheme::new(&account, 100_000_000, None, &[]).unwrap();
    for v in 0..2 {
        from(
            &mut account,
            v,
            &hub.says(v, &[hub.account_statement(5, true)]),
        );
    }
    let mut effects = Vec::new();
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    assert!(scheme.has_synced());
    (account, scheme)
}

/// The hub's word on `key`, as of `height`: it has no account for it.
fn no_account_for(key: dassie_identity::PublicKey, height: u64) -> Statement {
    Statement::Account {
        height,
        key,
        account: None,
    }
}

// spec/areas/hubs.md#when-the-history-is-gone: a `pay` of ours whose
// outcome the account never read is never cancelled on a guess. It stays
// on its way across a restart, and waits for f + 1 validators to say
// whether the hub has an account for the key it paid in, as of a block it
// had executed by: none as of an older block says nothing; none as of
// that block cancels it, and takes back through suspense what the
// account's state counted of it meanwhile.
#[test]
fn an_unread_pay_waits_for_the_hub_across_a_restart() {
    let hub = Hub::new();
    let (account, mut scheme) = synced_scheme(&hub);
    let settlement = Name::new("00112233445566778899aabbccddeeff").unwrap();
    let key = NodeKey::from_bytes(&[8; 32]).public_key();
    scheme.settling.insert(
        settlement,
        scheme::Settling {
            peer: Name::new("newcomer").unwrap(),
            drops: 5,
        },
    );
    scheme
        .unread_pays
        .insert(settlement, scheme::UnreadPay { key, height: 20 });
    scheme.check_invariants();
    // A restart.
    let state = scheme.state();
    let mut account = HubAccount::new(
        FIRST,
        hub.rules.clone(),
        Arc::clone(&hub.key),
        Destination {
            account: [5; 20],
            tag: None,
        },
        Name::new(SCHEME_ID).unwrap(),
        &account.state(),
    )
    .unwrap();
    let mut scheme = HubScheme::new(&account, 100_000_000, Some(scheme.balance()), &state).unwrap();
    assert_eq!(scheme.state(), state, "the scheme's state round-trips");
    let booked = scheme.balance();
    let mut effects = Vec::new();
    scheme.reconcile(Some(&mut account), &[(settlement, units(5))], &mut effects);
    assert!(effects.is_empty(), "not cancelled on a guess");
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    assert!(effects.is_empty());
    for v in 0..2 {
        from(&mut account, v, &hub.says(v, &[no_account_for(key, 10)]));
    }
    scheme.on_asset(0, &mut account, &mut effects);
    account.told();
    assert!(effects.is_empty(), "as of a block before it executed");
    assert!(scheme.unread_pays.contains_key(&settlement));
    for v in 0..2 {
        from(&mut account, v, &hub.says(v, &[no_account_for(key, 20)]));
    }
    scheme.on_asset(0, &mut account, &mut effects);
    assert_eq!(
        effects,
        [
            dassie_settlement::Effect::Cancel { settlement },
            dassie_settlement::Effect::Reconcile {
                amount: units(5),
                gain: false,
                cause: dassie_settlement::ReconcileCause::Unexplained,
            },
        ]
    );
    assert_eq!(scheme.balance(), booked - units(5));
    assert!(scheme.unread_pays.is_empty());
    assert!(scheme.settling.is_empty());
    scheme.check_invariants();
}

// The scheme's state of version 1, from before unread `pay`s were kept,
// reads as one with none.
#[test]
fn a_scheme_state_of_version_1_reads_with_no_unread_pays() {
    let hub = Hub::new();
    let (account, scheme) = synced_scheme(&hub);
    let state = scheme.state();
    assert_eq!(state.first(), Some(&2));
    let (body, count) = state.split_at(state.len() - 4);
    assert_eq!(count, [0; 4], "no unread pays");
    let mut old = body.to_vec();
    old[0] = 1;
    let restored = HubScheme::new(&account, 100_000_000, Some(scheme.balance()), &old).unwrap();
    assert_eq!(restored.state(), state);
}

/// What validators `who` say of the issuer's hold after the block at
/// `height`: frozen since `since`, if any.
fn issuer_says(
    hub: &Hub,
    account: &mut HubAccount,
    who: core::ops::Range<usize>,
    height: u64,
    since: Option<u64>,
) {
    let statement = Statement::Issuer {
        height,
        timestamp: 1_000 + height,
        frozen: since,
        held: since.is_some(),
    };
    for v in who {
        from(account, v, &hub.says(v, core::slice::from_ref(&statement)));
    }
}

// spec/decisions/token-hubs.md: the account knows the hub frozen once
// f + 1 validators say so, the newest they agree on, and its method is
// unavailable meanwhile; its window is the one the ledger reckons.
#[test]
fn a_freeze_is_read_from_what_f_plus_one_sign() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    issuer_says(&hub, &mut account, 0..1, 6, Some(1_006));
    assert_eq!(account.frozen(), None, "one alone may lie");
    issuer_says(&hub, &mut account, 1..2, 6, Some(1_006));
    assert_eq!(account.frozen(), Some(1_006));
    assert!(account.escrow_held());
    assert_eq!(account.status(), LedgerStatus::Syncing, "unavailable");
    issuer_says(&hub, &mut account, 0..2, 5, None);
    assert_eq!(account.frozen(), Some(1_006), "an older state says nothing");
    issuer_says(&hub, &mut account, 2..4, 8, None);
    assert_eq!(account.frozen(), None, "thawed");
    assert_eq!(account.status(), LedgerStatus::Synced);
    account.check_invariants();

    // The window is the ledger's.
    let mut token = crate::ledger::tests::Hub::with(|rules| {
        rules.terms = crate::rules::tests::token_terms(rules.terms.clone());
    });
    token.all_validators(|_| {
        crate::operation::ValidatorOperation::AttestIssuer(crate::operation::Issuer {
            ledger: 5,
            frozen: true,
            fee: false,
        })
    });
    let since = token.ledger.frozen().unwrap();
    assert_eq!(
        Some(account::freeze_window_end(token.ledger.terms(), since)),
        token.ledger.freeze_window_end()
    );
}

/// A claim on hold 2 for one commitment expiring at `expiry`.
fn claim_expiring(expiry: crate::rules::Millis) -> MemberOperation {
    claim_on(2, expiry)
}

/// A claim on hold `hold` for one commitment expiring at `expiry`.
fn claim_on(hold: u64, expiry: crate::rules::Millis) -> MemberOperation {
    let hold = crate::promise::HoldNumber(hold);
    let commitment = crate::promise::Commitment {
        hold,
        sequence: 1,
        amount: 5,
        condition: [3; 32],
        expiry,
    };
    let signed = crate::promise::Signed::sign(
        commitment,
        &Hub::new().rules.id(),
        &NodeKey::from_bytes(&[9; 32]),
    );
    MemberOperation::Claim {
        hold,
        fulfilled: alloc::vec![crate::promise::Fulfilled {
            commitment: signed,
            fulfillment: [4; 32],
        }],
    }
}

/// A redeem on hold `hold`.
fn redeem_on(hold: u64) -> MemberOperation {
    let claim = crate::promise::Claim {
        hold: crate::promise::HoldNumber(hold),
        total: 5,
        through: 1,
    };
    MemberOperation::Redeem(crate::promise::Signed::sign(
        claim,
        &Hub::new().rules.id(),
        &NodeKey::from_bytes(&[9; 32]),
    ))
}

/// A synced account whose settlement `s`, a `hold-fund`, is on its way
/// with sequence 3 when the hub freezes.
fn frozen_with_a_settlement_on_its_way(hub: &Hub) -> (HubAccount, Name, MemberOperation) {
    let mut account = synced(hub);
    let settlement = Name::new("s").unwrap();
    let fund = MemberOperation::HoldFund {
        hold: crate::promise::HoldNumber(1),
        amount: 1,
    };
    account.prepare(settlement, fund.clone()).unwrap();
    assert!(account.execute(settlement));
    let sent = submitted(&mut account);
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[0].sequence, 3);
    issuer_says(hub, &mut account, 0..2, 6, Some(1_006));
    (account, settlement, fund)
}

// spec/decisions/token-hubs.md: a frozen hub moves nothing, so no
// settlement or withdrawal is taken on, and a claim it would not take in
// the freeze's window goes back to the scheme, to claim what it holds.
#[test]
fn a_frozen_hub_takes_on_only_what_it_takes() {
    let hub = Hub::new();
    let (mut account, _, fund) = frozen_with_a_settlement_on_its_way(&hub);
    assert_eq!(
        account.prepare(Name::new("t").unwrap(), fund),
        Err(SchemeError::Unavailable),
        "no settlement while frozen"
    );
    let release = MemberOperation::HoldRelease {
        hold: crate::promise::HoldNumber(1),
    };
    assert_eq!(
        account.submit(Purpose::Withdraw, release),
        Err(SchemeError::Unavailable),
        "no withdrawal while frozen"
    );
    let window = account.freeze_window_end().unwrap();
    let hold = crate::promise::HoldNumber(2);
    account
        .submit(Purpose::Claim(hold), claim_expiring(window + 1))
        .unwrap();
    assert!(submitted(&mut account).is_empty());
    account.on_timer(1);
    assert!(!account.has(Purpose::Claim(hold)));
    assert!(account.news().contains(&News::Dropped {
        purpose: Purpose::Claim(hold),
    }));
    account.check_invariants();
}

// spec/decisions/token-hubs.md: while the hub is frozen, an operation on
// its way waits for the thaw, and a claim the hub takes goes beside it
// with its sequence. Whichever applies, the other can never apply after
// it: it is signed again with the next sequence.
#[test]
fn a_claim_goes_beside_an_operation_that_waits_for_the_thaw() {
    let hub = Hub::new();
    let (mut account, settlement, fund) = frozen_with_a_settlement_on_its_way(&hub);
    let hold = crate::promise::HoldNumber(2);
    let claim = claim_expiring(account.freeze_window_end().unwrap());
    account.submit(Purpose::Claim(hold), claim.clone()).unwrap();
    let sent = submitted(&mut account);
    assert_eq!(sent.len(), 4);
    assert_eq!((sent[0].sequence, &sent[0].operation), (3, &claim));
    account.check_invariants();
    // Sent again in time; what waits is not.
    account.on_timer(10 * account::RESUBMIT_MS);
    let again = submitted(&mut account);
    assert!(!again.is_empty());
    assert!(again.iter().all(|signed| signed.operation == claim));
    // The claim applies: the settlement can never apply with its sequence.
    let applied = Statement::Executed {
        height: 7,
        timestamp: 1_007,
        index: 0,
        operation: Operation::Member(sent[0].clone()),
        outcome: Outcome::Applied {
            fee: 10,
            detail: crate::ledger::Detail::Paid(5),
        },
    };
    let final_block = Statement::Final {
        height: 7,
        timestamp: 1_007,
    };
    for v in 0..3 {
        let answer = hub.says(v, &[applied.clone(), final_block.clone()]);
        from(&mut account, v, &answer);
    }
    assert_eq!(account.cursor(), 7);
    assert!(!account.has(Purpose::Claim(hold)));
    assert!(
        !account.was_sent(Purpose::Settle(settlement)),
        "never to apply"
    );
    assert!(account.has(Purpose::Settle(settlement)), "but kept");
    assert!(submitted(&mut account).is_empty(), "until the thaw");
    // After the thaw it goes, with the next sequence.
    issuer_says(&hub, &mut account, 0..2, 8, None);
    let sent = submitted(&mut account);
    assert_eq!(sent.len(), 4);
    assert_eq!((sent[0].sequence, &sent[0].operation), (4, &fund));
    account.check_invariants();
}

// The freeze is kept with the books, so a restart during it knows it; a
// state of version 1 knows none.
#[test]
fn the_freeze_is_kept_across_a_restart() {
    let hub = Hub::new();
    let mut account = synced(&hub);
    let restore = |state: &[u8]| {
        HubAccount::new(
            FIRST,
            hub.rules.clone(),
            Arc::clone(&hub.key),
            Destination {
                account: [5; 20],
                tag: None,
            },
            Name::new(SCHEME_ID).unwrap(),
            state,
        )
        .unwrap()
    };
    let state = account.state();
    assert_eq!(state.first(), Some(&2));
    assert_eq!(state.last(), Some(&0), "no issuer's hold known");
    let mut old = state[..state.len() - 1].to_vec();
    old[0] = 1;
    assert_eq!(restore(&old).state(), state);
    issuer_says(&hub, &mut account, 0..2, 6, Some(1_006));
    let restored = restore(&account.state());
    assert_eq!(restored.frozen(), Some(1_006));
    assert!(restored.escrow_held());
    assert_eq!(restored.state(), account.state());
}

// The account reckons what a frozen hub takes as its ledger does, for
// every kind of operation, and for claims within the window and past it.
#[test]
fn the_account_waits_for_what_the_ledger_does() {
    let mut token = crate::ledger::tests::Hub::with(|rules| {
        rules.terms = crate::rules::tests::token_terms(rules.terms.clone());
    });
    let destination = Destination {
        account: [5; 20],
        tag: None,
    };
    let hold = crate::promise::HoldNumber(2);
    let operations = |end: crate::rules::Millis| {
        alloc::vec![
            MemberOperation::Transfer {
                to: AccountNumber(9),
                amount: 5,
            },
            MemberOperation::Pay {
                key: NodeKey::from_bytes(&[8; 32]).public_key(),
                amount: 5,
            },
            MemberOperation::SetDisbursal(destination),
            MemberOperation::HoldOpen {
                payee: AccountNumber(9),
                amount: 5,
            },
            MemberOperation::HoldFund { hold, amount: 5 },
            MemberOperation::HoldReduce { hold, amount: 5 },
            MemberOperation::HoldRelease { hold },
            redeem_on(2),
            claim_on(2, end),
            claim_on(2, end + 1),
            MemberOperation::Withdraw {
                amount: 5,
                to: destination,
            },
        ]
    };
    for operation in &operations(1_000) {
        assert!(!token.ledger.waits(operation));
        assert!(!account::waits(None, operation));
    }
    token.all_validators(|_| {
        crate::operation::ValidatorOperation::AttestIssuer(crate::operation::Issuer {
            ledger: 5,
            frozen: true,
            fee: false,
        })
    });
    let window = token.ledger.freeze_window_end();
    let end = window.unwrap();
    let mut waiting = 0;
    for operation in &operations(end) {
        let waits = token.ledger.waits(operation);
        assert_eq!(account::waits(window, operation), waits, "{operation:?}");
        waiting += usize::from(waits);
    }
    assert_eq!(
        waiting,
        operations(end).len() - 1,
        "all but the claim within"
    );
}

// spec/decisions/token-hubs.md: a claim sent before the account knew of
// the freeze, holding a commitment past its window, goes back to the
// scheme once it can no longer apply before the thaw: when the hub
// refuses it for the freeze, or once the account read every block up to
// the one it knows the freeze as of. Its sequence was not used, so the
// claim the scheme makes of what the window holds goes with it.
#[test]
fn a_claim_sent_before_the_freeze_was_known_goes_back() {
    let hub = Hub::new();
    let hold = crate::promise::HoldNumber(2);
    let claimed = |account: &mut HubAccount| {
        let claim = claim_expiring(u64::MAX / 2);
        account.submit(Purpose::Claim(hold), claim).unwrap();
        let sent = submitted(account);
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0].sequence, 3);
        sent[0].clone()
    };
    let final_block = |height: u64| Statement::Final {
        height,
        timestamp: 1_000 + height,
    };

    // The hub refuses it for the freeze.
    let mut account = synced(&hub);
    let sent = claimed(&mut account);
    issuer_says(&hub, &mut account, 0..2, 8, Some(1_006));
    assert!(account.was_sent(Purpose::Claim(hold)), "it may still apply");
    let refused = Statement::Executed {
        height: 6,
        timestamp: 1_006,
        index: 0,
        operation: Operation::Member(sent),
        outcome: Outcome::Refused(crate::ledger::Refusal::Frozen),
    };
    for v in 0..3 {
        let answer = hub.says(v, &[refused.clone(), final_block(6)]);
        from(&mut account, v, &answer);
    }
    assert_eq!(account.cursor(), 6);
    assert!(!account.has(Purpose::Claim(hold)));
    assert!(account.news().contains(&News::Dropped {
        purpose: Purpose::Claim(hold),
    }));
    account.check_invariants();

    // The validators dropped it: no block up to the freeze's took it.
    let mut account = synced(&hub);
    claimed(&mut account);
    issuer_says(&hub, &mut account, 0..2, 6, Some(1_006));
    assert!(account.was_sent(Purpose::Claim(hold)), "not read up to it");
    for v in 0..3 {
        from(&mut account, v, &hub.says(v, &[final_block(6)]));
    }
    assert_eq!(account.cursor(), 6);
    assert!(!account.has(Purpose::Claim(hold)));
    assert!(account.news().contains(&News::Dropped {
        purpose: Purpose::Claim(hold),
    }));
    // What the window holds goes with the sequence it did not use.
    let within = claim_expiring(account.freeze_window_end().unwrap());
    account
        .submit(Purpose::Claim(hold), within.clone())
        .unwrap();
    let sent = submitted(&mut account);
    assert_eq!(sent.len(), 4);
    assert_eq!((sent[0].sequence, &sent[0].operation), (3, &within));
    account.check_invariants();
}

// spec/decisions/token-hubs.md: a frozen hub takes no redeem, and those
// that waited at the freeze go back to the scheme, so redeems on more
// holds than the queue has room for never keep out the claims it still
// pays. Settlements never take more than their share of the queue.
#[test]
fn a_frozen_hub_keeps_the_queue_for_claims() {
    let hub = Hub::new();
    let number = |n: usize| u64::try_from(n).unwrap();
    let mut account = synced(&hub);
    for hold in (0..account::MAX_QUEUED).map(|n| number(n) + 10) {
        let purpose = Purpose::Redeem(crate::promise::HoldNumber(hold));
        account.submit(purpose, redeem_on(hold)).unwrap();
    }
    let claim = Purpose::Claim(crate::promise::HoldNumber(2));
    assert_eq!(
        account.submit(claim, claim_on(2, 1)),
        Err(SchemeError::Unavailable),
        "full"
    );
    assert_eq!(submitted(&mut account).len(), 4, "one on its way");
    issuer_says(&hub, &mut account, 0..2, 6, Some(1_006));
    assert_eq!(account.queue.len(), 1, "only the one on its way is kept");
    let dropped = account
        .news()
        .iter()
        .filter(|news| {
            matches!(
                news,
                News::Dropped {
                    purpose: Purpose::Redeem(_)
                }
            )
        })
        .count();
    assert_eq!(dropped, account::MAX_QUEUED - 1);
    let redeem = Purpose::Redeem(crate::promise::HoldNumber(99));
    assert_eq!(
        account.submit(redeem, redeem_on(99)),
        Err(SchemeError::Unavailable),
        "no redeem while frozen"
    );
    let end = account.freeze_window_end().unwrap();
    for hold in (0..account::MAX_QUEUED - 1).map(|n| number(n) + 100) {
        let purpose = Purpose::Claim(crate::promise::HoldNumber(hold));
        account.submit(purpose, claim_on(hold, end)).unwrap();
    }
    account.check_invariants();

    // Settlements take at most their share of the queue, frozen or not.
    let mut account = synced(&hub);
    let fund = |n: usize| MemberOperation::HoldFund {
        hold: crate::promise::HoldNumber(number(n)),
        amount: 1,
    };
    for n in 0..account::MAX_QUEUED_WITHOUT_DEADLINE {
        let id = Name::new(&alloc::format!("s{n}")).unwrap();
        account.prepare(id, fund(n)).unwrap();
    }
    assert_eq!(
        account.prepare(Name::new("full").unwrap(), fund(99)),
        Err(SchemeError::Unavailable)
    );
    account.submit(claim, claim_on(2, 1)).unwrap();
    account.check_invariants();
}
