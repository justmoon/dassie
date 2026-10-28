//! Joining a hub by being paid, the node's side
//! (spec/designs/settlement-hubs.md#5-joining): a peering over a hub where
//! one end has no account in it carries no promises, the books alone take
//! its packets, and the payer pays the newcomer in once it owes it the
//! hub's minimum for a new account, never before.
//!
//! The scheme is the stub with commitments, standing in for a hub's member
//! that has an account and a peer without one, or the other way round
//! ([`Joiner`]).

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use dassie_identity::NodeId;
use dassie_ledger::{AccountName, LedgerId, Name, Reference, TransferSpec};
use dassie_settlement::{
    Commitments, Decided, Effect, ExternalAsset, Held, HoldsWith, Membership, PreparedSettlement,
    Promised, SchemeError, SettlementScheme, Stub,
};

use super::super::Core;
use super::super::hubs::{HubSent, Joining, paid_in_credit};
use super::{COMMITTING, NOW, add_stub, core, downlink_request, scheme_id};

/// The hub's minimum for a new account, in the stub's units.
const MINIMUM: u64 = 3_000_000_000;

/// The stub's name.
const STUB: Name = dassie_ledger::name!("stub");

/// A core over a [`Joiner`] of `side`, with a downlink peered on it whose
/// accounts are open: the downlink, and the amounts the joiner is asked to
/// settle.
fn joining_core(seed: u64, side: Side) -> (Core, NodeId, Arc<Mutex<Vec<u64>>>) {
    let mut core = core(seed);
    add_stub(&mut core, COMMITTING);
    let asked = Joiner::install(&mut core, side);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[11; 32]));
    let at = NOW + 1_000;
    core.begin_step(at);
    let body = downlink_request(&downlink, at);
    core.peer_inbound(dassie_peer::InboundId(1), downlink.identity(), &body, None);
    let peer = downlink.node_id();
    assert!(core.peer.peering_of(&peer).is_some());
    assert!(core.open_downlink_accounts(peer, STUB));
    core.finish_step(&mut outputs);
    (core, peer, asked)
}

/// Moves `amount` between the owner and `peer` in `core`'s books: what we
/// owe the peer grows by it if `owe`, and what it owes us otherwise, as
/// packets between us would.
fn book(core: &mut Core, peer: NodeId, amount: u64, owe: bool, at: u64) {
    let name = Name::new(peer.as_str()).unwrap();
    let interledger = core
        .account(LedgerId::StubUsd, AccountName::AssetsInterledger(name))
        .unwrap();
    let pool = core
        .account(LedgerId::StubUsd, AccountName::EquityOwner)
        .unwrap();
    let (debit, credit) = if owe {
        (pool, interledger)
    } else {
        (interledger, pool)
    };
    core.begin_step(at);
    core.ledger
        .create(
            at,
            &TransferSpec {
                debit,
                credit,
                amount,
                pending: false,
                reference: Reference::Deposit,
            },
        )
        .unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
}

/// The credit `core` extends to `peer` over the stub, and its settlements
/// to it still pending.
fn credit(core: &Core, peer: NodeId) -> (u64, u64) {
    let (credit, _, settling) = core.hub_books(STUB, peer).unwrap();
    (credit, settling)
}

/// Runs `core`'s settlement checks at `at`.
fn check_settlements(core: &mut Core, at: u64) {
    core.begin_step(at);
    core.settle.at_ms = 0;
    core.settlement_timers();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
}

// spec/designs/settlement-hubs.md#5-joining: a payer over a hub pays a peer
// without an account in once it owes it the hub's minimum for a new
// account, never before, and then with all it owes it; it extends the peer
// no credit, not even for that `pay` on its way, and asks it nothing, since
// it can pay nothing.
// Once the `pay` executed, it is booked as our settlement, and the peering
// is one over the hub as any.
#[test]
fn a_payer_pays_a_newcomer_in_at_the_minimum_and_not_before() {
    let (mut core, peer, asked) = joining_core(41, Side::Payer);
    let scheme = scheme_id("stub");
    let name = Name::new(peer.as_str()).unwrap();
    assert_eq!(core.joining(STUB, name), Some(Joining::Paying(MINIMUM)));
    assert_eq!(credit(&core, peer), (0, 0), "nothing extended to it");
    check_settlements(&mut core, NOW + 2_000);
    assert!(!core.settle.asked.contains_key(&(peer, scheme)));
    // Short of the minimum, nothing is paid, and the scheme is not asked.
    book(&mut core, peer, MINIMUM - 1, true, NOW + 3_000);
    core.begin_step(NOW + 4_000);
    assert_eq!(core.settlement_requested(peer, scheme), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(asked.lock().unwrap().is_empty());
    // Past the minimum, all it owes, and no credit for the `pay` on its
    // way: the peer cannot spend it again meanwhile.
    let owed = MINIMUM + 500;
    book(&mut core, peer, owed - (MINIMUM - 1), true, NOW + 5_000);
    core.begin_step(NOW + 6_000);
    assert_eq!(core.settlement_requested(peer, scheme), owed);
    assert_eq!(*asked.lock().unwrap(), [owed]);
    assert_eq!(credit(&core, peer), (0, owed));
    assert!(core.owed(peer, scheme).unwrap() <= 0);
    outputs.clear();
    core.finish_step(&mut outputs);
    // Executed, it is posted once, and the peer has its account: the credit
    // is its hold, nothing.
    assert_eq!(credit(&core, peer), (0, 0));
    assert_eq!(core.joining(STUB, name), None);
    assert!(core.ledger.pending_transfers().next().is_none());
    assert_eq!(core.owed(peer, scheme), Some(0));
    assert_eq!(settled_out(&core), u128::from(owed), "booked once");
    // From then on, a Prepare without a commitment is refused: our books
    // count the peer's hold toward us as paid.
    core.begin_step(NOW + 7_000);
    let packet = prepare_packet();
    assert!(!core.check_hub_prepare(NOW + 7_000, &peer, scheme, 9, &packet, None));
    outputs.clear();
    core.finish_step(&mut outputs);
}

/// A Prepare of 1000, as the peer protocol carries it.
fn prepare_packet() -> Vec<u8> {
    let prepare = dassie_ilp::Prepare {
        amount: 1_000,
        expires_at: dassie_ilp::Timestamp::from_unix_ms(NOW + 30_000),
        execution_condition: [3; 32],
        destination: dassie_ilp::Address::parse("test.das.somewhere").unwrap(),
        data: &[],
    };
    let mut packet = vec![0; 256];
    let length = dassie_oer::Encode::encode(&prepare, &mut packet);
    packet.truncate(length);
    packet
}

/// What `core`'s books paid out of the stub's account as settlements.
fn settled_out(core: &Core) -> u128 {
    let settlement = core
        .account(LedgerId::StubUsd, AccountName::AssetsSettlement)
        .unwrap();
    core.ledger
        .account(settlement)
        .unwrap()
        .balance()
        .credits_posted
}

// spec/designs/settlement-hubs.md#5-joining: to a peer without an account,
// a Prepare goes without a commitment, and the scheme holds nothing of it;
// from it, one without a commitment, or with one we did not expect, is the
// books' to judge, so the peer spends only what we owe it.
#[test]
fn without_an_account_packets_go_without_promises() {
    let (mut core, peer, _) = joining_core(42, Side::Payer);
    let scheme = scheme_id("stub");
    let packet = prepare_packet();
    core.begin_step(NOW + 2_000);
    let sent = core.send_over_hub(NOW + 2_000, (&peer, scheme), 5, &packet, 28_000);
    assert_eq!(sent, Some(HubSent::Sent));
    let mut held = Vec::new();
    core.with_commitments(STUB, |c| c.held(&mut held));
    assert!(held.is_empty(), "no promise was made: {held:?}");
    for commitment in [None, Some(&b"unexpected"[..])] {
        assert!(core.check_hub_prepare(NOW + 2_000, &peer, scheme, 6, &packet, commitment));
    }
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
}

// spec/designs/settlement-hubs.md#5-joining: a member without an account
// extends the peer that pays it in the credit to owe it the hub's minimum
// for a new account and its trigger besides, asks it to settle once it owes
// the minimum, not before, whatever the hold the peer has not opened, and
// settles nothing itself; paid in, the `pay` is booked as the peer's
// settlement, once, and the credit follows the holds from then on.
#[test]
fn a_newcomer_trusts_its_payer_with_the_minimum_and_asks_at_it() {
    let (mut core, peer, asked) = joining_core(43, Side::Newcomer);
    let scheme = scheme_id("stub");
    let name = Name::new(peer.as_str()).unwrap();
    assert_eq!(core.joining(STUB, name), Some(Joining::PaidIn(MINIMUM)));
    let extended = paid_in_credit(MINIMUM, core.settle.trigger);
    assert_eq!(credit(&core, peer), (extended, 0));
    // Advertised with no account, so that members with one peer with it,
    // once the offers follow the scheme's membership.
    core.update_peer_schemes();
    let offered = core
        .peer
        .offers()
        .iter()
        .find_map(|offer| offer.hub)
        .unwrap();
    assert_eq!(offered.account, Membership::NO_ACCOUNT);
    // Owed a little short of the minimum, it asks nothing.
    book(&mut core, peer, MINIMUM - 1, false, NOW + 2_000);
    check_settlements(&mut core, NOW + 3_000);
    assert!(!core.settle.asked.contains_key(&(peer, scheme)));
    book(&mut core, peer, 1, false, NOW + 4_000);
    check_settlements(&mut core, NOW + 5_000);
    assert!(core.settle.asked.contains_key(&(peer, scheme)));
    // Asked, it has nothing to pay with, and the scheme is not asked.
    core.begin_step(NOW + 6_000);
    assert_eq!(core.settlement_requested(peer, scheme), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(asked.lock().unwrap().is_empty());
    // Paid in: booked once, and the credit follows the holds, as far as
    // what the peer may still owe us allows.
    core.begin_step(NOW + 7_000);
    core.incoming_settlement(peer, scheme, MINIMUM, &[])
        .unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert_eq!(core.joining(STUB, name), None);
    assert_eq!(core.owed(peer, scheme), Some(0));
    let (credit_now, _) = credit(&core, peer);
    assert!(credit_now < extended, "{credit_now} of {extended}");
    let settlement = core
        .account(LedgerId::StubUsd, AccountName::AssetsSettlement)
        .unwrap();
    let balance = core.ledger.account(settlement).unwrap().balance();
    assert_eq!(
        balance.debits_posted,
        u128::from(dassie_settlement::STUB_INITIAL_BALANCE) + u128::from(MINIMUM)
    );
}

/// The pretend hub of a [`Joiner`].
const JOINER_HUB: [u8; 32] = *b"the joiners' pretend hub, ok..!!";

/// Which end of a peering over a hub a [`Joiner`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// It has an account; its peer has none, and is paid in by it.
    Payer,
    /// It has none; its peer, which has one, pays it in.
    Newcomer,
}

/// The stub with commitments as a hub's member at one end of a peering
/// with a member that has no account in the hub, or as the one without:
/// until the newcomer is paid in, neither commits
/// ([`Commitments::commits_with`]), the payer settles at least the hub's
/// minimum for a new account, and the hold the stub funds with that `pay`
/// is no hold. What it does otherwise is the stub's.
struct Joiner {
    stub: RefCell<Stub>,
    side: Side,
    /// Whether the newcomer has its account now.
    opened: Cell<bool>,
    /// The payer's `pay`, while it is prepared.
    paying: Option<Name>,
    /// What the `pay` moved, which the stub took for a hold.
    paid: u64,
    /// The amounts it was asked to settle, which the test reads.
    asked: Arc<Mutex<Vec<u64>>>,
}

impl Joiner {
    /// Puts a joiner of `side` in the place of `core`'s stub with
    /// commitments: the amounts it is asked to settle.
    fn install(core: &mut Core, side: Side) -> Arc<Mutex<Vec<u64>>> {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let stub = Stub::new(COMMITTING, None, &[]).unwrap();
        let joiner = Self {
            stub: RefCell::new(stub),
            side,
            opened: Cell::new(false),
            paying: None,
            paid: 0,
            asked: Arc::clone(&asked),
        };
        core.schemes.get_mut(&STUB).unwrap().scheme = Box::new(joiner);
        asked
    }

    fn joining(&self) -> bool {
        !self.opened.get()
    }

    fn with_inner<R>(&self, f: impl FnOnce(&mut dyn Commitments) -> R) -> R {
        f(self.stub.borrow_mut().commitments().unwrap())
    }
}

impl SettlementScheme for Joiner {
    fn id(&self) -> Name {
        dassie_ledger::name!("stub")
    }

    fn ledger(&self) -> LedgerId {
        LedgerId::StubUsd
    }

    fn balance(&self) -> u64 {
        self.stub.borrow().balance()
    }

    fn state(&self) -> Vec<u8> {
        self.stub.borrow().state()
    }

    fn state_version(&self) -> u64 {
        self.stub.borrow().state_version()
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        self.stub.borrow().peering_info(out);
    }

    fn create_peering_request(
        &mut self,
        peer: Name,
        info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        self.stub.get_mut().create_peering_request(peer, info, out)
    }

    fn accept_peering_request(&mut self, peer: Name, data: &[u8]) -> Result<Vec<u8>, SchemeError> {
        self.stub.get_mut().accept_peering_request(peer, data)
    }

    fn finalize_peering_request(
        &mut self,
        peer: Name,
        info: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        self.stub.get_mut().finalize_peering_request(peer, info)
    }

    /// The payer pays the newcomer in with at least the minimum, never
    /// less; the newcomer has nothing to pay with.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn rand_core::CryptoRng,
        asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        self.asked.lock().unwrap().push(amount);
        if self.joining() && (self.side == Side::Newcomer || amount < MINIMUM) {
            return Err(SchemeError::Unavailable);
        }
        let prepared = self
            .stub
            .get_mut()
            .prepare_settlement(rng, asset, peer, amount, state)?;
        if self.joining() {
            self.paying = Some(prepared.id);
        }
        Ok(prepared)
    }

    fn discard_settlement(&mut self, asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        self.stub.get_mut().discard_settlement(asset, settlement);
    }

    /// The `pay` opens the newcomer's account: from then on, the peering
    /// is one over the hub as any.
    fn execute_settlement(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        let before = self.balance();
        self.stub
            .get_mut()
            .execute_settlement(asset, settlement, effects);
        if self.paying == Some(settlement) {
            self.paying = None;
            self.paid += before - self.balance();
            self.opened.set(true);
        }
    }

    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        self.stub.get_mut().reconcile(asset, pending, effects);
    }

    /// The newcomer is paid in: its account opens.
    fn handle_settlement(
        &mut self,
        peer: Name,
        amount: u64,
        data: &[u8],
        state: &[u8],
        effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        self.stub
            .get_mut()
            .handle_settlement(peer, amount, data, state, effects)?;
        if self.side == Side::Newcomer && self.joining() {
            self.paid += amount;
            self.opened.set(true);
        }
        Ok(())
    }

    fn deposit(&mut self, amount: u64, effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        self.stub.get_mut().deposit(amount, effects)
    }

    fn commitments(&mut self) -> Option<&mut dyn Commitments> {
        Some(self)
    }

    fn check_invariants(&self) {
        self.stub.borrow().check_invariants();
    }
}

impl Commitments for Joiner {
    fn commit(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        assert!(!self.joining(), "no promise while one of us has no account");
        self.with_inner(|inner| inner.commit(now_ms, peer, request_id, promised, out))
    }

    fn check_commitment(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        commitment: &[u8],
    ) -> Result<(), SchemeError> {
        assert!(!self.joining(), "no promise while one of us has no account");
        self.with_inner(|inner| {
            inner.check_commitment(now_ms, peer, request_id, promised, commitment)
        })
    }

    fn on_fulfilled(&mut self, now_ms: u64, peer: Name, request_id: u32, fulfillment: [u8; 32]) {
        self.with_inner(|inner| inner.on_fulfilled(now_ms, peer, request_id, fulfillment));
    }

    fn on_released(&mut self, peer: Name, request_id: u32, release: &mut Vec<u8>) {
        self.with_inner(|inner| inner.on_released(peer, request_id, release));
    }

    fn release_refused(
        &mut self,
        now_ms: u64,
        peer: Name,
        commitment: &[u8],
        release: &mut Vec<u8>,
    ) {
        self.with_inner(|inner| inner.release_refused(now_ms, peer, commitment, release));
    }

    fn on_rejected(&mut self, peer: Name, request_id: u32, release: &[u8]) -> bool {
        self.with_inner(|inner| inner.on_rejected(peer, request_id, release))
    }

    fn claim(&mut self, now_ms: u64, peer: Name, request_id: u32, taken: bool, out: &mut Vec<u8>) {
        self.with_inner(|inner| inner.claim(now_ms, peer, request_id, taken, out));
    }

    fn on_claim(&mut self, now_ms: u64, peer: Name, request_id: u32, claim: &[u8]) {
        self.with_inner(|inner| inner.on_claim(now_ms, peer, request_id, claim));
    }

    fn on_hub_wait(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        condition: [u8; 32],
        backstop_ms: u64,
    ) {
        self.with_inner(|inner| {
            inner.on_hub_wait(now_ms, peer, request_id, condition, backstop_ms);
        });
    }

    fn on_answered(&mut self, peer: Name, request_id: u32) {
        self.with_inner(|inner| inner.on_answered(peer, request_id));
    }

    fn take_decided(&mut self, out: &mut Vec<Decided>) {
        self.with_inner(|inner| inner.take_decided(out));
    }

    /// The stub's holds, less what the `pay` moved, which funds no hold.
    fn holds(&self, peer: Name) -> HoldsWith {
        let holds = self.with_inner(|inner| inner.holds(peer));
        match self.side {
            Side::Payer => HoldsWith {
                toward: holds.toward.saturating_sub(self.paid),
                ..holds
            },
            Side::Newcomer => HoldsWith {
                from: holds.from.saturating_sub(self.paid),
                ..holds
            },
        }
    }

    fn room_from(&self, peer: Name) -> Option<u64> {
        self.with_inner(|inner| inner.room_from(peer))
    }

    /// With no account while the newcomer has none.
    fn membership(&self) -> Option<Membership> {
        let account = match self.side {
            Side::Newcomer if self.joining() => Membership::NO_ACCOUNT,
            _ => 7,
        };
        Some(Membership {
            hub: JOINER_HUB,
            account,
        })
    }

    fn commits_with(&self, _peer: Name) -> bool {
        !self.joining()
    }

    fn opening_minimum(&self, _peer: Name) -> Option<u64> {
        (self.side == Side::Payer && self.joining()).then_some(MINIMUM)
    }

    fn paid_in_minimum(&self, _peer: Name) -> Option<u64> {
        (self.side == Side::Newcomer && self.joining()).then_some(MINIMUM)
    }

    fn held(&self, out: &mut Vec<(Name, Held)>) -> bool {
        self.with_inner(|inner| inner.held(out))
    }
}
