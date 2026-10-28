//! What the node sees of its settlement hubs, for live subscribers
//! (spec/areas/hubs.md#in-the-node): what each hub account read, in the
//! order it read it, the commitments and claims of the packets over a hub,
//! and the node's own moves between an owner book and a hub.
//!
//! Events are made only while someone listens, and wait in a bounded queue
//! until the step is on disk: past it, the oldest go, and are counted, so
//! a subscriber that falls behind hears how many it missed, and nothing is
//! ever buffered for it without bound.

use std::collections::VecDeque;

use dassie_hub::ledger::Outcome;
use dassie_hub::member::{News, Purpose};
use dassie_hub::operation::{CloseTo, MemberOperation, SignedMemberOperation};
use dassie_hub::promise::{Claim, Commitment, Signed};
use dassie_ledger::Name;

use super::hub_funding::{Direction, Move, Standing};

/// Most events waiting for the step to be on disk: past it, the oldest are
/// dropped and counted.
pub const MAX_HUB_EVENTS: usize = 1024;

/// Most moves remembered as told late: one on its way per hub.
const MAX_LATE_MOVES: usize = 16;

/// One thing the node saw of the hub of `scheme`, at `observed_at_ms` by
/// its own clock. Amounts are the hub's own, at six decimals (drops of XRP,
/// or millionths of a token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubEvent {
    /// The settlement scheme over the hub, which names it.
    pub scheme: Name,
    /// When the node saw it.
    pub observed_at_ms: u64,
    /// What it saw.
    pub kind: HubEventKind,
}

/// What the node saw.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubEventKind {
    /// A final block, up to which the account read everything that touched
    /// it.
    Block {
        /// Its height.
        height: u64,
        /// Its timestamp, by the hub's clock.
        timestamp_ms: u64,
    },
    /// An operation that touched the account, in a final block.
    Operation(HubOperation),
    /// The account as of a block through which it read everything.
    Account(HubAccountState),
    /// The hub has no account for the node's key.
    Unfunded,
    /// A hold's reduction took effect: `amount` went back to its payer.
    Reduced {
        /// The hold.
        hold: u64,
        /// What went back.
        amount: u64,
    },
    /// What claims took and waited in a hold was paid to its payee.
    ClaimsPaid {
        /// The hold.
        hold: u64,
        /// How many commitments' sequences it paid above the last redeemed
        /// W, and the first and last of them.
        count: u32,
        /// The first sequence paid, 0 if none.
        first: u64,
        /// The last sequence paid, 0 if none.
        last: u64,
        /// Whether it paid what waited unlisted.
        unlisted: bool,
    },
    /// The account read its holds anew as of a block, skipping what it no
    /// longer could read in order.
    Resynced {
        /// The block's height.
        height: u64,
        /// Its timestamp.
        timestamp_ms: u64,
        /// How many holds it mirrored before.
        holds_before: u32,
        /// The operation of ours that used its sequence in between, whose
        /// fate is not known.
        unread: Option<Purpose>,
    },
    /// An operation of ours the hub refused for good.
    Dropped(Purpose),
    /// A commitment of a packet over the hub: ours as the payer, signed, or
    /// a peer's we accepted as the payee.
    Commitment(HubPromise),
    /// A claim in the reply to a Fulfill: ours as the payer, sent, or a
    /// peer's as the payee, taken.
    Claim(HubPromise),
    /// A move of the node's between the owner book and the hub.
    Move(HubMoveEvent),
}

/// An operation that touched the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubOperation {
    /// The block's height.
    pub height: u64,
    /// The block's timestamp.
    pub timestamp_ms: u64,
    /// What it was, as the hub's log names it (`hold-open`, `claim`, ...).
    pub kind: &'static str,
    /// The account that signed it.
    pub signer: u32,
    /// The signer's sequence it used.
    pub sequence: u64,
    /// The hold it acts on or opened, if any.
    pub hold: Option<u64>,
    /// The other account it pays or opened, if any.
    pub peer_account: Option<u32>,
    /// What it moves: the amount it names; T for a `redeem`; what the
    /// commitments of a `claim` add up to; 0 for the rest.
    pub amount: u64,
    /// What became of it.
    pub outcome: Outcome,
    /// Why we submitted it, if it is ours.
    pub ours: Option<Purpose>,
}

/// The account's state, as of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubAccountState {
    /// The block's height.
    pub height: u64,
    /// The account's number, once known.
    pub number: Option<u32>,
    /// What it can spend.
    pub balance: u64,
    /// Its account reserve.
    pub reserve: u64,
    /// The sequence its next operation carries.
    pub sequence: u64,
    /// Everything deposits credited it, and withdrawals that came back.
    pub deposited: u64,
    /// Everything its withdrawals took from it.
    pub withdrawn: u64,
    /// What withdrawals that came back credited it.
    pub returned: u64,
    /// Whether it is closing.
    pub closing: bool,
}

/// Which side of a packet over a hub the node is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// It pays: it signs commitments and claims.
    Payer,
    /// It is paid: it takes them.
    Payee,
}

/// A commitment or a claim of one packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubPromise {
    /// Which side we are.
    pub role: Role,
    /// The peer, by its node ID.
    pub peer: Name,
    /// The hold it draws on.
    pub hold: u64,
    /// A commitment's sequence; a claim's W.
    pub sequence: u64,
    /// A commitment's amount; a claim's T.
    pub amount: u64,
    /// A commitment's condition; zero for a claim.
    pub condition: [u8; 32],
    /// A commitment's expiry, by the hub's clock; 0 for a claim.
    pub expiry_ms: u64,
}

impl HubPromise {
    /// The commitment `bytes` encode, if they do.
    #[must_use]
    pub fn commitment(role: Role, peer: Name, bytes: &[u8]) -> Option<Self> {
        let mut reader = dassie_oer::Reader::new(bytes);
        let Commitment {
            hold,
            sequence,
            amount,
            condition,
            expiry,
        } = Signed::<Commitment>::decode(&mut reader).ok()?.value;
        Some(Self {
            role,
            peer,
            hold: hold.0,
            sequence,
            amount,
            condition,
            expiry_ms: expiry,
        })
    }

    /// The claim `bytes` encode, if they do.
    #[must_use]
    pub fn claim(role: Role, peer: Name, bytes: &[u8]) -> Option<Self> {
        let mut reader = dassie_oer::Reader::new(bytes);
        let Claim {
            hold,
            total,
            through,
        } = Signed::<Claim>::decode(&mut reader).ok()?.value;
        Some(Self {
            role,
            peer,
            hold: hold.0,
            sequence: through,
            amount: total,
            condition: [0; 32],
            expiry_ms: 0,
        })
    }
}

/// Where a move of the node's is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveStage {
    /// Its payment went out.
    Started,
    /// The hub account's counters show it.
    Done,
    /// The side it leaves says its payment moved nothing.
    Failed,
    /// On its way for long: the owner is told.
    Late,
    /// The owner gave it up.
    GivenUp,
}

/// A move of the node's between the owner book and the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubMoveEvent {
    /// Where it is.
    pub stage: MoveStage,
    /// Which way.
    pub direction: Direction,
    /// How much, at six decimals.
    pub amount: u64,
    /// Its payment's ID with the scheme of the side it leaves.
    pub payment: Name,
    /// Whether the owner asked for it.
    pub owner: bool,
}

/// What a subscriber hears.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubStreamEvent {
    /// Something the node saw.
    Event(HubEvent),
    /// This many events were dropped because nobody took them in time.
    Lagged(u64),
}

/// The queue of events, which is fed only while someone listens.
#[derive(Debug, Default)]
pub struct HubTap {
    on: bool,
    events: VecDeque<HubEvent>,
    dropped: u64,
    /// The payments of the moves told late, so each is told once.
    late: Vec<Name>,
}

impl HubTap {
    /// Whether events are made: only while someone listens.
    #[must_use]
    pub const fn is_on(&self) -> bool {
        self.on
    }

    /// Turns making events on or off. Off, what waits is dropped.
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.events.clear();
            self.dropped = 0;
            self.late.clear();
        }
    }

    /// Queues `event`, dropping the oldest when full; nothing while off.
    pub fn push(&mut self, event: HubEvent) {
        if !self.on {
            return;
        }
        if self.events.len() >= MAX_HUB_EVENTS {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(event);
    }

    /// The next event to hand over: how many were dropped first, if any.
    pub fn poll(&mut self) -> Option<HubStreamEvent> {
        if self.dropped > 0 {
            return Some(HubStreamEvent::Lagged(std::mem::take(&mut self.dropped)));
        }
        self.events.pop_front().map(HubStreamEvent::Event)
    }

    /// Whether the move of `payment` is to be told late: only once.
    fn first_late(&mut self, payment: Name) -> bool {
        if self.late.contains(&payment) {
            return false;
        }
        if self.late.len() >= MAX_LATE_MOVES {
            self.late.remove(0);
        }
        self.late.push(payment);
        true
    }

    /// The bounds, and nothing kept while off.
    pub fn check_invariants(&self) {
        assert!(self.events.len() <= MAX_HUB_EVENTS);
        assert!(self.late.len() <= MAX_LATE_MOVES);
        assert!(self.on || (self.events.is_empty() && self.dropped == 0 && self.late.is_empty()));
    }
}

/// The event of `news`, if it is one a subscriber hears: the answers to
/// the account's lookups of other keys are its own business.
#[must_use]
pub fn of_news(news: &News, number: Option<u32>) -> Option<HubEventKind> {
    Some(match news {
        News::Final { height, timestamp } => HubEventKind::Block {
            height: *height,
            timestamp_ms: *timestamp,
        },
        News::Executed {
            height,
            timestamp,
            operation,
            outcome,
            ours,
        } => HubEventKind::Operation(operation_of(
            *height, *timestamp, operation, *outcome, *ours,
        )),
        News::Account { height, view } => HubEventKind::Account(HubAccountState {
            height: *height,
            number,
            balance: view.balance,
            reserve: view.reserve,
            sequence: view.sequence,
            deposited: view.deposited,
            withdrawn: view.withdrawn,
            returned: view.returned,
            closing: view.closing,
        }),
        News::Unfunded => HubEventKind::Unfunded,
        News::Reduced { hold, amount } => HubEventKind::Reduced {
            hold: hold.0,
            amount: *amount,
        },
        News::ClaimsPaid {
            hold,
            sequences,
            unlisted,
        } => HubEventKind::ClaimsPaid {
            hold: hold.0,
            count: u32::try_from(sequences.len()).unwrap_or(u32::MAX),
            first: sequences.iter().copied().min().unwrap_or(0),
            last: sequences.iter().copied().max().unwrap_or(0),
            unlisted: *unlisted,
        },
        News::Resynced {
            height,
            timestamp,
            before,
            unread,
        } => HubEventKind::Resynced {
            height: *height,
            timestamp_ms: *timestamp,
            holds_before: u32::try_from(before.len()).unwrap_or(u32::MAX),
            unread: unread.as_ref().map(|(purpose, _)| *purpose),
        },
        News::Dropped { purpose } => HubEventKind::Dropped(*purpose),
        News::Member { .. } => return None,
    })
}

/// An executed operation, summed up.
fn operation_of(
    height: u64,
    timestamp_ms: u64,
    signed: &SignedMemberOperation,
    outcome: Outcome,
    ours: Option<Purpose>,
) -> HubOperation {
    use dassie_hub::ledger::Detail;
    let opened = match outcome {
        Outcome::Applied { detail, .. } => Some(detail),
        _ => None,
    };
    let opened_hold = match opened {
        Some(Detail::Hold(hold)) => Some(hold.0),
        _ => None,
    };
    let opened_account = match opened {
        Some(Detail::Account(account)) => Some(account.0),
        _ => None,
    };
    let (kind, hold, peer_account, amount) = match &signed.operation {
        MemberOperation::Transfer { to, amount } => ("transfer", None, Some(to.0), *amount),
        MemberOperation::Pay { amount, .. } => ("pay", None, opened_account, *amount),
        MemberOperation::SetDisbursal(_) => ("set-disbursal", None, None, 0),
        MemberOperation::HoldOpen { payee, amount } => {
            ("hold-open", opened_hold, Some(payee.0), *amount)
        }
        MemberOperation::HoldFund { hold, amount } => ("hold-fund", Some(hold.0), None, *amount),
        MemberOperation::HoldReduce { hold, amount } => {
            ("hold-reduce", Some(hold.0), None, *amount)
        }
        MemberOperation::HoldRelease { hold } => ("hold-release", Some(hold.0), None, 0),
        MemberOperation::Redeem(claim) => {
            ("redeem", Some(claim.value.hold.0), None, claim.value.total)
        }
        MemberOperation::Claim { hold, fulfilled } => {
            let amount = fulfilled
                .iter()
                .fold(0u64, |sum, f| sum.saturating_add(f.commitment.value.amount));
            ("claim", Some(hold.0), None, amount)
        }
        MemberOperation::Withdraw { amount, .. } => ("withdraw", None, None, *amount),
        MemberOperation::Close(to) => {
            let peer = match to {
                CloseTo::Account(account) => Some(account.0),
                CloseTo::Ledger(_) => None,
            };
            ("close", None, peer, 0)
        }
        MemberOperation::Dispute { hold, .. } => ("dispute", Some(hold.0), None, 0),
    };
    HubOperation {
        height,
        timestamp_ms,
        kind,
        signer: signed.account.0,
        sequence: signed.sequence,
        hold,
        peer_account,
        amount,
        outcome,
        ours,
    }
}

impl super::Core {
    /// Turns the hub events on while someone listens, and off otherwise.
    pub fn set_hub_events(&mut self, on: bool) {
        self.hub_tap.set_on(on);
    }

    /// Queues what the node saw of the hub of `scheme`, now.
    pub(super) fn hub_event(&mut self, scheme: Name, kind: HubEventKind) {
        let observed_at_ms = self.now_ms;
        self.hub_tap.push(HubEvent {
            scheme,
            observed_at_ms,
            kind,
        });
    }

    /// Queues a commitment or claim of a packet over the hub of `scheme`,
    /// from its bytes, if anyone listens and they decode.
    pub(super) fn tap_promise(
        &mut self,
        scheme: Name,
        (role, peer): (Role, Name),
        bytes: &[u8],
        claim: bool,
    ) {
        if !self.hub_tap.is_on() || bytes.is_empty() {
            return;
        }
        let kind = if claim {
            HubPromise::claim(role, peer, bytes).map(HubEventKind::Claim)
        } else {
            HubPromise::commitment(role, peer, bytes).map(HubEventKind::Commitment)
        };
        if let Some(kind) = kind {
            self.hub_event(scheme, kind);
        }
    }

    /// A peer's claim we took, told for the hub whose account mirrors the
    /// hold it is signed for.
    pub(super) fn tap_claim_received(&mut self, peer: Name, bytes: &[u8], hubs: &[Name]) {
        if !self.hub_tap.is_on() {
            return;
        }
        let Some(promise) = HubPromise::claim(Role::Payee, peer, bytes) else {
            return;
        };
        let hold = dassie_hub::promise::HoldNumber(promise.hold);
        let scheme = hubs.iter().copied().find(|scheme| {
            self.hub_account(*scheme)
                .is_some_and(|account| account.hold(hold).is_some())
        });
        if let Some(scheme) = scheme {
            self.hub_event(scheme, HubEventKind::Claim(promise));
        }
    }

    /// Queues where a move of the hub of `scheme` is: `stage`, if anyone
    /// listens.
    pub(super) fn tap_move(&mut self, scheme: Name, stage: MoveStage, moved: &Move) {
        let kind = HubEventKind::Move(HubMoveEvent {
            stage,
            direction: moved.direction,
            amount: moved.drops,
            payment: moved.payment,
            owner: moved.owner,
        });
        self.hub_event(scheme, kind);
    }

    /// After the move of `scheme` that was on its way, `before`, was
    /// followed: whether it is done, failed, or late now.
    pub(super) fn tap_followed(&mut self, scheme: Name, before: Option<Move>, standing: &Standing) {
        if !self.hub_tap.is_on() {
            return;
        }
        let now = self.now_ms;
        let Some(funding) = self.funding.get(&scheme) else {
            return;
        };
        let (after, late) = (funding.moving, funding.late(now));
        match (before, after) {
            (Some(moved), None) => {
                let counter = standing.held.map(|held| match moved.direction {
                    Direction::Deposit => held.deposited,
                    Direction::Withdraw => held.withdrawn,
                });
                let stage = if counter.is_some_and(|counter| counter >= moved.target) {
                    MoveStage::Done
                } else {
                    MoveStage::Failed
                };
                self.tap_move(scheme, stage, &moved);
            }
            (_, Some(moving)) if late && self.hub_tap.first_late(moving.payment) => {
                self.tap_move(scheme, MoveStage::Late, &moving);
            }
            _ => {}
        }
    }

    /// Queues what the hub account on `asset` read since its schemes were
    /// last told, in the order it read it, if anyone listens.
    pub(super) fn tap_hub_news(&mut self, asset: dassie_settlement::AssetId) {
        if !self.hub_tap.is_on() {
            return;
        }
        let Some(active) = self.assets.get(&asset) else {
            return;
        };
        let any: &dyn core::any::Any = active.asset.as_ref();
        let Some(account) = any.downcast_ref::<dassie_hub::member::HubAccount>() else {
            return;
        };
        if account.news().is_empty() {
            return;
        }
        // A hub account's ID is its scheme's (spec/areas/hubs.md#in-the-node).
        let Ok(scheme) = Name::new(&asset.to_string()) else {
            return;
        };
        let number = account.number().map(|number| number.0);
        let kinds: Vec<HubEventKind> = account
            .news()
            .iter()
            .filter_map(|news| of_news(news, number))
            .collect();
        for kind in kinds {
            self.hub_event(scheme, kind);
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use dassie_hub::client::AccountView;
    use dassie_hub::ledger::{Detail, Failure};
    use dassie_hub::operation::AccountNumber;
    use dassie_hub::promise::HoldNumber;
    use dassie_identity::{NodeKey, Signature};

    use super::*;

    fn name(text: &str) -> Name {
        Name::new(text).unwrap()
    }

    fn signed(operation: MemberOperation) -> SignedMemberOperation {
        SignedMemberOperation {
            account: AccountNumber(7),
            sequence: 3,
            max_fee: 10,
            operation,
            signature: Signature::from_bytes([0; 64]),
        }
    }

    fn event(kind: HubEventKind) -> HubEvent {
        HubEvent {
            scheme: name("hub-xrpl-testnet-xrp"),
            observed_at_ms: 1,
            kind,
        }
    }

    #[test]
    fn a_final_block_is_its_height_and_stamp() {
        let news = News::Final {
            height: 12,
            timestamp: 1_700_000_000_000,
        };
        assert_eq!(
            of_news(&news, Some(4)),
            Some(HubEventKind::Block {
                height: 12,
                timestamp_ms: 1_700_000_000_000
            })
        );
    }

    #[test]
    fn an_executed_hold_open_names_the_hold_it_opened_and_its_payee() {
        let news = News::Executed {
            height: 5,
            timestamp: 99,
            operation: signed(MemberOperation::HoldOpen {
                payee: AccountNumber(9),
                amount: 1_000,
            }),
            outcome: Outcome::Applied {
                fee: 2,
                detail: Detail::Hold(HoldNumber(41)),
            },
            ours: Some(Purpose::Settle(name("s1"))),
        };
        let Some(HubEventKind::Operation(operation)) = of_news(&news, Some(7)) else {
            panic!("an operation");
        };
        assert_eq!(operation.kind, "hold-open");
        assert_eq!((operation.signer, operation.sequence), (7, 3));
        assert_eq!(operation.hold, Some(41));
        assert_eq!(operation.peer_account, Some(9));
        assert_eq!(operation.amount, 1_000);
        assert_eq!(operation.ours, Some(Purpose::Settle(name("s1"))));
    }

    #[test]
    fn a_failed_redeem_keeps_its_hold_and_total() {
        let claim = Signed {
            value: Claim {
                hold: HoldNumber(3),
                total: 500,
                through: 8,
            },
            signature: Signature::from_bytes([0; 64]),
        };
        let news = News::Executed {
            height: 6,
            timestamp: 100,
            operation: signed(MemberOperation::Redeem(claim)),
            outcome: Outcome::Failed {
                fee: 1,
                reason: Failure::Claim,
            },
            ours: None,
        };
        let Some(HubEventKind::Operation(operation)) = of_news(&news, None) else {
            panic!("an operation");
        };
        assert_eq!(
            (operation.kind, operation.hold, operation.amount),
            ("redeem", Some(3), 500)
        );
        assert!(matches!(operation.outcome, Outcome::Failed { .. }));
    }

    #[test]
    fn the_account_state_carries_its_counters_and_number() {
        let key = NodeKey::from_bytes(&[1; 32]).public_key();
        let news = News::Account {
            height: 8,
            view: AccountView {
                key,
                balance: 10,
                reserve: 2,
                sequence: 4,
                disbursal: true,
                closing: false,
                deposited: 30,
                withdrawn: 15,
                returned: 1,
            },
        };
        let Some(HubEventKind::Account(state)) = of_news(&news, Some(7)) else {
            panic!("a state");
        };
        assert_eq!(state.number, Some(7));
        assert_eq!(
            (
                state.balance,
                state.reserve,
                state.deposited,
                state.withdrawn
            ),
            (10, 2, 30, 15)
        );
        assert_eq!(state.returned, 1);
    }

    #[test]
    fn claims_paid_sum_up_their_sequences() {
        let news = News::ClaimsPaid {
            hold: HoldNumber(2),
            sequences: vec![9, 4, 6],
            unlisted: false,
        };
        assert_eq!(
            of_news(&news, None),
            Some(HubEventKind::ClaimsPaid {
                hold: 2,
                count: 3,
                first: 4,
                last: 9,
                unlisted: false
            })
        );
    }

    #[test]
    fn lookups_are_not_events() {
        let key = NodeKey::from_bytes(&[1; 32]).public_key();
        let news = News::Member {
            key,
            account: None,
            height: 1,
        };
        assert_eq!(of_news(&news, None), None);
    }

    #[test]
    fn promises_decode_from_their_bytes() {
        let commitment = Signed {
            value: Commitment {
                hold: HoldNumber(5),
                sequence: 11,
                amount: 70,
                condition: [8; 32],
                expiry: 1_234,
            },
            signature: Signature::from_bytes([0; 64]),
        };
        let bytes = dassie_oer::Encode::to_vec(&commitment);
        let peer = name("peer");
        let promise = HubPromise::commitment(Role::Payer, peer, &bytes).unwrap();
        assert_eq!(
            (
                promise.hold,
                promise.sequence,
                promise.amount,
                promise.expiry_ms
            ),
            (5, 11, 70, 1_234)
        );
        assert_eq!(promise.condition, [8; 32]);
        let claim = Signed {
            value: Claim {
                hold: HoldNumber(5),
                total: 300,
                through: 11,
            },
            signature: Signature::from_bytes([0; 64]),
        };
        let bytes = dassie_oer::Encode::to_vec(&claim);
        let promise = HubPromise::claim(Role::Payee, peer, &bytes).unwrap();
        assert_eq!((promise.amount, promise.sequence), (300, 11));
        assert_eq!(HubPromise::claim(Role::Payee, peer, &[1, 2]), None);
    }

    #[test]
    fn the_tap_is_bounded_and_counts_what_it_dropped() {
        let mut tap = HubTap::default();
        tap.push(event(HubEventKind::Unfunded));
        assert_eq!(tap.poll(), None, "nothing is kept while nobody listens");
        tap.set_on(true);
        for _ in 0..MAX_HUB_EVENTS + 5 {
            tap.push(event(HubEventKind::Unfunded));
            tap.check_invariants();
        }
        assert_eq!(tap.poll(), Some(HubStreamEvent::Lagged(5)));
        let mut left = 0;
        while let Some(HubStreamEvent::Event(_)) = tap.poll() {
            left += 1;
        }
        assert_eq!(left, MAX_HUB_EVENTS);
        tap.push(event(HubEventKind::Unfunded));
        tap.set_on(false);
        tap.check_invariants();
        assert_eq!(tap.poll(), None);
    }
}
