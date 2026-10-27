//! Settling with peers (spec/decisions/own-fulfills.md,
//! spec/areas/settlement.md#asking-a-peer-to-settle): a node asks a peer to
//! settle once the peer is close to the credit it extends, and settles half
//! the way to its own trigger when a peer asks it. A settlement is prepared,
//! booked as a durable pending transfer, executed once that is on disk, and
//! announced to the peer once it executed. Incoming settlements are booked
//! before the scheme sees them. Each settlement method of a peering does
//! all of it on its own ledger: its own position, credit, requests and
//! backoff, while the settlement channel, one message at a time, is the
//! pair's (spec/decisions/settlement-methods.md).

use std::collections::BTreeMap;

use dassie_identity::NodeId;
use dassie_ledger::{
    AccountName, AccountPath, LedgerId, Name, Reference, Transfer, TransferId, TransferSpec,
};
use dassie_peer::Room;
use dassie_peer::wire::SchemeId;
use dassie_settlement::{DollarRate, LedgerStatus, policy};

use super::events::SettlementEvent;
use super::{Core, EngineError};
use crate::node::Policies;

/// The first wait after a cancelled settlement; it doubles with each one
/// in a row, up to ten minutes, so a node without funds does not journal a
/// pair of rows every few seconds, and one with funds again settles soon.
const CANCEL_BACKOFF_MIN_MS: u64 = 1_000;
const CANCEL_BACKOFF_MAX_MS: u64 = 600_000;

/// How often a scheme that cannot settle says so, at most.
const UNAVAILABLE_WARN_MS: u64 = 10 * 60 * 1_000;

/// A lie a node tells its peers, in a simulation of a dishonest network:
/// its own code stays honest otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lie {
    /// It never settles what it owes.
    NeverSettle,
    /// As a payer over a settlement hub, it sends its payee no claims: its
    /// replies to the payee's Fulfills are empty, so the
    /// payee must claim on the hub (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    NoClaims,
}

/// The settling side of the core.
pub(super) struct Settlements {
    /// When what is owed to peers is checked next.
    pub at_ms: u64,
    pub interval_ms: u64,
    /// How often a peer that did not settle since is asked again, at most.
    pub request_every_ms: u64,
    /// Our trigger for every peer, and the least we settle, from the
    /// largest packet (spec/decisions/thresholds-in-packets.md).
    pub trigger: u64,
    pub minimum: u64,
    /// The most credit a peer can earn (spec/decisions/earned-trust.md).
    pub credit_cap: u64,
    /// Where reading the fees peers earned us stands.
    pub growth: super::growth::Growth,
    /// When each peer was last asked to settle on each method, until it
    /// settles on it (spec/decisions/settlement-methods.md).
    pub asked: BTreeMap<(NodeId, SchemeId), u64>,
    /// Our settlements prepared and not yet final, by scheme and ID.
    pub settling: BTreeMap<(Name, Name), Settling>,
    /// Settlements to execute once their pending transfers are on disk.
    pub to_execute: Vec<(Name, Name)>,
    /// Peers not settled with on a method before a time, after cancelled
    /// settlements on it, and how many in a row: a method whose ledger
    /// cannot pay holds up no other.
    pub backoff: BTreeMap<(NodeId, SchemeId), (u64, u32)>,
    /// Settlements that executed while an earlier one to the same peer was
    /// still being announced: posted and announced once that one arrives.
    pub announce: Vec<(Name, Name)>,
    /// When each scheme last said it cannot settle.
    pub unavailable_warned: BTreeMap<Name, u64>,
    /// Whether this node lies that it never settles, in a simulation.
    pub never_settles: bool,
    /// Whether this node, as a payer over a hub, sends no claims, in a
    /// simulation.
    pub sends_no_claims: bool,
    /// The secret its XRPL accounts check validations under, in a
    /// simulation, rather than their signatures.
    #[cfg(feature = "testing")]
    pub validation_secret: Option<[u8; 32]>,
    /// The largest packet, which schemes that count in packets need.
    pub max_packet: u64,
    /// The timings of a dollar token's peg guard.
    pub peg: dassie_xrpl::peg::Timings,
}

impl Settlements {
    pub(super) const fn new(now_ms: u64, policies: &Policies) -> Self {
        Self {
            at_ms: now_ms.saturating_add(policies.settlement_interval_ms),
            interval_ms: policies.settlement_interval_ms,
            request_every_ms: policies.settlement_request_every_ms,
            trigger: policy::trigger(policies.connector.max_packet_amount),
            minimum: policy::minimum(policies.connector.max_packet_amount),
            credit_cap: policy::credit_cap(policies.connector.max_packet_amount),
            max_packet: policies.connector.max_packet_amount,
            peg: policies.peg,
            growth: super::growth::Growth::new(),
            asked: BTreeMap::new(),
            settling: BTreeMap::new(),
            to_execute: Vec::new(),
            backoff: BTreeMap::new(),
            announce: Vec::new(),
            unavailable_warned: BTreeMap::new(),
            never_settles: false,
            sends_no_claims: false,
            #[cfg(feature = "testing")]
            validation_secret: None,
        }
    }
}

/// One of our settlements, prepared and not yet final.
#[derive(Clone, Debug)]
pub(super) struct Settling {
    /// The peer it pays.
    pub peer: NodeId,
    /// Its pending transfer.
    pub transfer: TransferId,
    /// How much.
    pub amount: u64,
    /// What the scheme gave to send the peer.
    pub message: Vec<u8>,
}

/// A peer's position, as the settlement policy needs it.
struct Position {
    scheme: Name,
    ledger: LedgerId,
    position: u128,
    credit_out: u64,
}

impl Core {
    /// Makes this node tell `lie`, in a simulation of a dishonest network.
    #[cfg(feature = "testing")]
    pub fn lie(&mut self, lie: Lie) {
        match lie {
            Lie::NeverSettle => self.settle.never_settles = true,
            Lie::NoClaims => self.settle.sends_no_claims = true,
        }
    }

    /// Has this node's XRPL accounts, those it has and those to come, check
    /// validations by the fake ledger's keyed hash under `secret`, as a
    /// simulation does, rather than by their signatures
    /// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
    #[cfg(feature = "testing")]
    pub fn check_validations_with(&mut self, secret: [u8; 32]) {
        self.settle.validation_secret = Some(secret);
        for active in self.assets.values_mut() {
            super::schemes::check_validations_with(active.asset.as_mut(), secret);
        }
    }

    /// This node's XRPL accounts, for a simulation to count what they
    /// proved and caught
    /// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
    #[cfg(feature = "testing")]
    pub fn xrpl_accounts(&self) -> impl Iterator<Item = &dassie_xrpl::XrplAccount> {
        self.assets.values().filter_map(|active| {
            let any: &dyn core::any::Any = active.asset.as_ref();
            any.downcast_ref::<dassie_xrpl::XrplAccount>()
        })
    }

    /// Whether the books hold a packet's money in flight, for a simulation
    /// to aim a fault at.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn packets_in_flight(&self) -> bool {
        self.ledger
            .pending_transfers()
            .any(|transfer| matches!(transfer.reference, Reference::Packet(_)))
    }

    /// Whether the books hold a settlement of ours not yet final: sent, or
    /// applied by the ledger before the node heard of it. Until it is final,
    /// the books and the account differ, so a simulation judges neither.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn settlements_in_flight(&self) -> bool {
        self.ledger
            .pending_transfers()
            .any(|transfer| matches!(transfer.reference, Reference::Settlement { .. }))
    }

    /// Whether the books hold a settlement of ours to `node` not yet final,
    /// for a simulation to aim a fault at.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn settling_with(&self, node: &NodeId) -> bool {
        self.ledger
            .pending_transfers()
            .any(|transfer| self.settles_to(transfer, node))
    }

    /// Checks every peer, asking those near the credit we extend to settle,
    /// and announces settlements that waited for an earlier one.
    pub(super) fn settlement_timers(&mut self) {
        let now = self.now_ms;
        if self.settle.at_ms > now {
            return;
        }
        self.settle.at_ms = now.saturating_add(self.settle.interval_ms);
        self.grow_credit();
        self.check_schemes();
        self.report_room();
        self.ask_peers();
        if self.settle.never_settles {
            return;
        }
        self.announce_deferred();
    }

    /// Tells the peer protocol which peers our books leave short of room
    /// for a packet while we can ask them to settle: one that stays short
    /// past the grace, settling nothing, is withdrawn as stuck rather than
    /// waiting for a routine settlement
    /// (spec/decisions/peering-lifecycle.md). Each settlement booked from it
    /// starts the grace again. While our scheme does not see its ledger, we
    /// cannot ask: nothing is judged, and the time short neither grows nor
    /// starts again. While our requests to settle surely never reach the
    /// peer, or no settlement from it can reach us, the peer protocol counts
    /// the time toward withdrawing it, not toward ending the peering for
    /// room. Each method of a peering is judged on its own, on its own
    /// ledger (spec/decisions/settlement-methods.md).
    fn report_room(&mut self) {
        let now = self.now_ms;
        let peers = self.methods_of_active_peers();
        for (node, scheme) in peers {
            // A downlink is in no link state, so withdrawing it would only
            // stop the payments to it that raise its room: one that owes
            // nothing is never withdrawn or ended for room
            // (spec/areas/network.md#advertised-peers).
            let owes_nothing = self.peer.is_downlink(&node)
                && self.owed(node, scheme).is_some_and(|owed| owed >= 0);
            // A peer we are to pay into a hub can owe us nothing: we extend
            // it nothing until it has an account there
            // (spec/designs/settlement-hubs.md#5-joining).
            let joining = self.joining_on(&node, scheme);
            let paying = matches!(joining, Some(super::hubs::Joining::Paying(_)));
            let room = if !self.sees_ledger_joining(scheme, joining) {
                Room::Unjudged
            } else if owes_nothing || paying {
                Room::Enough
            } else if self
                .position(node, scheme)
                // `minimum` is one of the largest packets
                // (spec/decisions/thresholds-in-packets.md).
                .is_some_and(|at| at.position < u128::from(self.settle.minimum))
            {
                // Two checks' worth at most: more is a stall of ours.
                let most_ms = self.settle.interval_ms.saturating_mul(2);
                // While no settlement from the peer can reach us, it is
                // withdrawn in time, but never ended for room it could not
                // restore (spec/decisions/ended-for-room.md).
                if self.can_receive(scheme) {
                    Room::Short { most_ms }
                } else {
                    Room::ShortUnpayable { most_ms }
                }
            } else {
                Room::Enough
            };
            self.peer.room(now, &node, scheme, room);
        }
    }

    /// Whether `scheme` sees its ledger, so that a peer on it can be asked
    /// to settle.
    fn sees_ledger(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str()).ok().is_some_and(|name| {
            self.schemes
                .get(&name)
                .is_some_and(|active| self.scheme_status(active) == LedgerStatus::Synced)
        })
    }

    /// [`sees_ledger`](Self::sees_ledger), for a peering that stands as
    /// `joining`: a scheme the hub has no account for yet, to be paid in by
    /// the peer, waits to be funded, yet sees the hub, and reads the `pay`
    /// that opens its account as the peer's settlement
    /// (spec/designs/settlement-hubs.md#5-joining).
    fn sees_ledger_joining(&self, scheme: SchemeId, joining: Option<super::hubs::Joining>) -> bool {
        matches!(joining, Some(super::hubs::Joining::PaidIn(_))) || self.sees_ledger(scheme)
    }

    /// Whether peers on `scheme` trust each other without limit, so that
    /// we ask none to settle on our own account, and settle with none, not
    /// even when asked at a peering's end
    /// (spec/areas/settlement.md#the-stub-scheme).
    fn unlimited(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str()).ok().is_some_and(|name| {
            self.schemes
                .get(&name)
                .is_some_and(|active| active.unlimited)
        })
    }

    /// Whether a peer's settlement on `scheme` can reach us now: not while
    /// our line cannot receive (spec/areas/settlement.md#when-peers-cannot-pay-us).
    fn can_receive(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str()).ok().is_some_and(|name| {
            self.schemes
                .get(&name)
                .is_some_and(|active| active.scheme.can_receive())
        })
    }

    /// The settlement check of each scheme that settles through an asset:
    /// what it reads of its ledger each time, such as an issuer's account,
    /// is asked for now, and comes as news. Each is first handed the rates
    /// it reads, as the node has them, such as XRP's for a dollar token's
    /// peg guard.
    fn check_schemes(&mut self) {
        let now = self.now_ms;
        let schemes: Vec<Name> = self.schemes.keys().copied().collect();
        for scheme in schemes {
            let rates = &self.prices.rates;
            let Some(active) = self.schemes.get_mut(&scheme) else {
                continue;
            };
            for code in active.scheme.rate_currencies() {
                let rate = rates.get(code).zip(rates.as_of(code));
                let rate = rate.map(|(per_dollar, as_of_ms)| DollarRate {
                    per_dollar,
                    precision: dassie_connector::RATE_PRECISION,
                    as_of_ms,
                });
                active.scheme.on_rate(code, rate);
            }
            let Some((active, Some(asset))) = self.scheme_and_asset(scheme) else {
                continue;
            };
            active.scheme.on_check(now, asset);
            if let Some(asset) = self.schemes.get(&scheme).and_then(|active| active.asset) {
                self.carry_out(asset);
            }
        }
        // A guard that went without a reading for too long pauses its
        // scheme with no news from its ledger.
        if self.follow_pegs() {
            self.update_peer_schemes();
        }
    }

    /// Follows each scheme's peg guard and whether its ledger converts:
    /// once a guard paused or resumed its scheme, the owner is told, and
    /// once a ledger stopped or started converting, a hub's with its owner
    /// book's guard too, the prices are built again at the commit, without
    /// its ledger or with it, and the owner's views with them
    /// (spec/decisions/issued-assets.md). Returns whether a guard changed.
    pub(super) fn follow_pegs(&mut self) -> bool {
        // Which ledgers a hub's owner book halts, before the schemes are
        // walked to be changed (`Core::converts_now`).
        let halted = LedgerId::ALL.map(|ledger| self.hub_halted(ledger));
        let mut changed = Vec::new();
        for (id, active) in &mut self.schemes {
            let ledger = active.scheme.ledger();
            let by_hub = LedgerId::ALL
                .iter()
                .zip(halted)
                .any(|(on, halted)| *on == ledger && halted);
            let converts = active.converts_now() && !by_hub;
            let paused = active.scheme.peg().and_then(|peg| peg.paused);
            if paused != active.paused {
                changed.push((*id, paused));
                active.paused = paused;
            }
            if converts != active.converts {
                active.converts = converts;
                self.prices.due = true;
                self.changes.ledger = true;
            }
        }
        let any = !changed.is_empty();
        for (scheme, paused) in changed {
            self.changes.ledger = true;
            self.report(match paused {
                Some(why) => SettlementEvent::PegPaused { scheme, why },
                None => SettlementEvent::PegResumed { scheme },
            });
        }
        any
    }

    /// Asks each peer that is close to the credit we extend to settle:
    /// again at the next check once our books show a settlement from it,
    /// and otherwise at most once per [`Settlements::request_every_ms`],
    /// for a peer that does not answer. Only our own books decide; the peer
    /// decides how much (spec/decisions/own-fulfills.md).
    ///
    /// A peer is asked only while our scheme sees its ledger: until then,
    /// what the peer pays is not booked, and it would be asked again. A
    /// peering is made on a scheme only once it first synced
    /// (spec/decisions/synced-before-peering.md), which booked what the
    /// account held as the owner's deposit, so what a peer settles when
    /// asked, here or by a node ending the peering
    /// (spec/decisions/ending-a-peering.md), is booked as its settlement.
    fn ask_peers(&mut self) {
        let now = self.now_ms;
        let every = self.settle.request_every_ms;
        let peers = self.methods_of_active_peers();
        self.settle.asked.retain(|key, _| peers.contains(key));
        for (node, scheme) in peers {
            // While one of us has no account in the hub, there are no holds:
            // a peer we are to pay in cannot settle, and the one that pays
            // us in is asked by the books alone
            // (spec/designs/settlement-hubs.md#5-joining).
            let joining = self.joining_on(&node, scheme);
            // Nor while its settlement cannot reach us: it would only fail,
            // and cost the peer a fee. Nor ever with unlimited trust.
            if !self.sees_ledger_joining(scheme, joining)
                || !self.can_receive(scheme)
                || self.unlimited(scheme)
                || matches!(joining, Some(super::hubs::Joining::Paying(_)))
            {
                continue;
            }
            let due = self
                .settle
                .asked
                .get(&(node, scheme))
                .is_none_or(|at| at.saturating_add(every) <= now);
            let close = self
                .position(node, scheme)
                .is_some_and(|at| policy::ask(at.position, self.settle.trigger))
                || joining.is_none()
                    && self
                        .hub_room(node, scheme)
                        .is_some_and(|room| policy::ask(u128::from(room), self.settle.trigger));
            if due && close && self.peer.request_settlement(now, &node, scheme).is_ok() {
                self.settle.asked.insert((node, scheme), now);
            }
        }
        self.after_peer();
    }

    /// Each method of each active peering, by peer and then in the order
    /// the pair agreed them: at most [`dassie_peer::MAX_METHODS`] a peer.
    fn methods_of_active_peers(&self) -> Vec<(NodeId, SchemeId)> {
        self.peer
            .active_peers()
            .flat_map(|(id, peering)| peering.schemes().map(move |scheme| (*id, scheme)))
            .collect()
    }

    /// What `node`'s hold toward us over the hub of `scheme` has room for,
    /// as our scheme reads it; None on a peering not over a hub. Over a hub
    /// the peer can send us only what its hold has room for, so we ask it
    /// to settle once that runs low, even while the books, which net what
    /// we owe it the other way, show room enough
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    fn hub_room(&mut self, node: NodeId, scheme: SchemeId) -> Option<u64> {
        let scheme = Name::new(scheme.as_str()).ok()?;
        if !self.schemes.get(&scheme)?.over_hub {
            return None;
        }
        let peer = Name::new(node.as_str()).ok()?;
        self.with_commitments(scheme, |c| c.room_from(peer))?
    }

    /// A peer asks us to settle on `scheme`: we settle half the way to our
    /// own trigger for it, unless a settlement to it is on its way already,
    /// which is the answer (spec/decisions/own-fulfills.md).
    ///
    /// Returns what is then on its way to the peer, which the answer tells
    /// it (spec/decisions/ending-a-peering.md): a settlement already under
    /// way, which is the whole answer, or the one made now.
    pub(super) fn settlement_requested(&mut self, node: NodeId, scheme: SchemeId) -> u64 {
        // With unlimited trust, nothing is ever settled, nor on its way.
        if self.settle.never_settles || self.unlimited(scheme) {
            return 0;
        }
        let under_way = self.settling_to(&node, scheme);
        if under_way > 0 || !self.can_settle(&node, scheme) {
            return under_way;
        }
        // Only on a method of the peering, with what its scheme keeps for
        // the peer on it.
        let Some(state) = self
            .peer
            .peering_of(&node)
            .and_then(|peering| peering.method(scheme))
            .map(|method| method.state.clone())
        else {
            return 0;
        };
        // While one of us has no account in its hub, we pay the peer in once
        // we owe it the hub's minimum for a new account, never before, with
        // what we owe it; without an account, we have nothing to pay with
        // (spec/designs/settlement-hubs.md#5-joining).
        let joining = self.joining_on(&node, scheme);
        if matches!(joining, Some(super::hubs::Joining::PaidIn(_))) {
            return 0;
        }
        let Some((mut at, mut over_hub, mut amount)) = self.requested(node, scheme) else {
            return 0;
        };
        if let Some(super::hubs::Joining::Paying(minimum)) = joining {
            amount = self.pay_in_amount(&at, minimum);
        }
        // Over a hub, what we settle funds our hold toward the peer: value
        // moves with it, which opens a downlink's accounts, and then stands
        // where they do. Without room for them, nothing is settled
        // (spec/decisions/lazy-downlink-accounts.md).
        if over_hub && amount > 0 && !self.has_accounts(node, at.ledger) {
            if !self.open_downlink_accounts(node, at.scheme) {
                return 0;
            }
            let Some(opened) = self.requested(node, scheme) else {
                return 0;
            };
            (at, over_hub, amount) = opened;
            if let Some(super::hubs::Joining::Paying(minimum)) = joining {
                amount = self.pay_in_amount(&at, minimum);
            }
        }
        if amount > 0 {
            self.prepare_settlement(node, &at, amount, &state, over_hub);
        }
        self.settling_to(&node, scheme)
    }

    /// Where we stand with `node` on `scheme`, whether it is over a hub, and
    /// what we would settle to it now when asked.
    fn requested(&self, node: NodeId, scheme: SchemeId) -> Option<(Position, bool, u64)> {
        let at = self.position(node, scheme)?;
        let over_hub = self
            .schemes
            .get(&at.scheme)
            .is_some_and(|active| active.over_hub);
        let position = if over_hub {
            hub_position(&at, self.peer_credit)
        } else {
            at.position
        };
        let amount = self.whole_units(
            &at.scheme,
            policy::requested_amount(position, self.settle.trigger, self.settle.minimum),
        );
        Some((at, over_hub, amount))
    }

    /// What we pay a peer into its hub with, standing at `at` with it: what
    /// we owe it net, in whole units of the ledger, once that is at least
    /// the hub's minimum for a new account, `minimum`, and nothing before.
    /// So whoever cycles fresh keys through us pays each one's minimum
    /// itself (spec/decisions/hubs-only.md). All of it, not the minimum
    /// alone: once the peer has its account, what we owed it beyond the
    /// `pay` no claim could ever cover, since its packets came without
    /// promises.
    fn pay_in_amount(&self, at: &Position, minimum: u64) -> u64 {
        let position = i128::try_from(at.position).unwrap_or(i128::MAX);
        let owed = u64::try_from(position - i128::from(at.credit_out)).unwrap_or(0);
        let owed = self.whole_units(&at.scheme, owed);
        if owed >= minimum { owed } else { 0 }
    }

    /// Whether `node` has accounts on `ledger`.
    fn has_accounts(&self, node: NodeId, ledger: LedgerId) -> bool {
        Name::new(node.as_str()).is_ok_and(|name| {
            self.ledger
                .find(&AccountPath::new(
                    ledger,
                    AccountName::AssetsInterledger(name),
                ))
                .is_some()
        })
    }

    /// What is on its way to `node` on `scheme`, in its ledger's units: our
    /// settlements still pending in that ledger (prepared, executing, or
    /// waiting to be announced), and the one announced on it that the node
    /// has not yet acknowledged. Other schemes' are in other units, and are
    /// not the answer.
    fn settling_to(&self, node: &NodeId, scheme: SchemeId) -> u64 {
        let ledger = Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .map(|active| active.scheme.ledger());
        let pending: u64 = self
            .ledger
            .pending_transfers()
            .filter(|transfer| self.settles_to(transfer, node))
            .filter(|transfer| {
                self.ledger
                    .account(transfer.debit)
                    .is_some_and(|account| Some(account.path().ledger) == ledger)
            })
            .map(|transfer| transfer.amount)
            .sum();
        let announced = self
            .peer
            .channel(node)
            .and_then(|channel| channel.outbox.as_ref())
            .filter(|outbox| outbox.scheme == scheme)
            .map_or(0, |outbox| outbox.amount);
        pending.saturating_add(announced)
    }

    /// `amount`, less what `scheme`'s ledger cannot move.
    fn whole_units(&self, scheme: &Name, amount: u64) -> u64 {
        let unit = self
            .schemes
            .get(scheme)
            .map_or(1, |active| active.scheme.unit().max(1));
        amount - amount % unit
    }

    /// Whether we may settle with `node` on its method on `scheme` now: we
    /// hold a channel with it, and no message of ours is on its way over
    /// it, the channel being the pair's; and on that method no settlement
    /// of ours is in progress or still pending from before a restart, and
    /// no cancelled one holds it off (spec/decisions/settlement-methods.md).
    pub(super) fn can_settle(&self, node: &NodeId, scheme: SchemeId) -> bool {
        let on = |of: &Name| of.as_str() == scheme.as_str();
        self.peer.channel(node).is_some()
            && !self.peer.settling(node)
            && !self
                .settle
                .settling
                .iter()
                .any(|((of, _), settling)| settling.peer == *node && on(of))
            && !self.pending_settlement_to(node, scheme)
            && self
                .settle
                .backoff
                .get(&(*node, scheme))
                .is_none_or(|(until, _)| *until <= self.now_ms)
    }

    /// Whether a settlement transfer to `node` on `scheme` is pending in
    /// the ledger.
    fn pending_settlement_to(&self, node: &NodeId, scheme: SchemeId) -> bool {
        self.ledger.pending_transfers().any(|transfer| {
            matches!(transfer.reference, Reference::Settlement { scheme: of, .. }
                if of.as_str() == scheme.as_str())
                && self.settles_to(transfer, node)
        })
    }

    /// Whether `transfer` is a settlement of ours to `node`.
    fn settles_to(&self, transfer: &Transfer, node: &NodeId) -> bool {
        settles_to(&self.ledger, transfer, node)
    }

    /// The credit we extend to `node` on `scheme`'s ledger now.
    pub(super) fn credit_out(&self, node: NodeId, scheme: SchemeId) -> Option<u64> {
        self.position(node, scheme).map(|at| at.credit_out)
    }

    /// Where we stand with `node` on `scheme`'s ledger.
    fn position(&self, node: NodeId, scheme: SchemeId) -> Option<Position> {
        let scheme = Name::new(scheme.as_str()).ok()?;
        let ledger = self.schemes.get(&scheme)?.scheme.ledger();
        let name = Name::new(node.as_str()).ok()?;
        let interledger = self.ledger.find(&AccountPath::new(
            ledger,
            AccountName::AssetsInterledger(name),
        ));
        let trust = self
            .ledger
            .find(&AccountPath::new(ledger, AccountName::ContraTrust(name)));
        let (Some(interledger), Some(trust)) = (interledger, trust) else {
            // A downlink nothing moved with yet has no accounts: it stands
            // where one with nothing on them would, owing nothing and owed
            // nothing, and extended no credit
            // (spec/decisions/lazy-downlink-accounts.md).
            return Some(Position {
                scheme,
                ledger,
                position: 0,
                credit_out: 0,
            });
        };
        Some(Position {
            scheme,
            ledger,
            position: policy::position(self.ledger.account(interledger)?.balance()),
            credit_out: policy::trust(self.ledger.account(trust)?.balance()),
        })
    }

    /// What we owe `node` net on `scheme`'s ledger: negative when it owes
    /// us.
    pub(super) fn owed(&self, node: NodeId, scheme: SchemeId) -> Option<i128> {
        let at = self.position(node, scheme)?;
        let position = i128::try_from(at.position).ok()?;
        Some(position - i128::from(at.credit_out))
    }

    /// Prepares a settlement and books its pending transfer; it executes
    /// once that is on disk. Over a hub, it funds our hold toward
    /// the peer, which is part of the credit we extend to it: the credit is
    /// raised by it first, so that the books take it, and follows what the
    /// scheme prepared, or nothing, at once (spec/areas/hubs.md#in-the-node).
    fn prepare_settlement(
        &mut self,
        node: NodeId,
        at: &Position,
        amount: u64,
        state: &[u8],
        over_hub: bool,
    ) {
        let Ok(name) = Name::new(node.as_str()) else {
            return;
        };
        if !over_hub {
            self.prepare_and_book(node, name, at, amount, state);
            return;
        }
        // A `pay` into the hub funds no hold: it is the peer's once it
        // executes, and we owe it that much already, so the books take it
        // without credit (spec/designs/settlement-hubs.md#5-joining).
        if matches!(
            self.joining(at.scheme, name),
            Some(super::hubs::Joining::Paying(_))
        ) {
            self.prepare_and_book(node, name, at, amount, state);
            self.sync_credit_with(at.scheme, node);
            return;
        }
        if let Err(error) =
            super::growth::raise_trust(&mut self.ledger, self.now_ms, at.ledger, name, amount)
        {
            self.report(SettlementEvent::BooksCannotSettle { node, error });
            return;
        }
        self.prepare_and_book(node, name, at, amount, state);
        self.sync_credit_with(at.scheme, node);
    }

    // Panics only on a broken invariant: what was validated is taken.
    #[allow(clippy::panic)]
    fn prepare_and_book(
        &mut self,
        node: NodeId,
        name: Name,
        at: &Position,
        amount: u64,
        state: &[u8],
    ) {
        let (Ok(interledger), Ok(settlement)) = (
            self.account(at.ledger, AccountName::AssetsInterledger(name)),
            self.account(at.ledger, AccountName::AssetsSettlement),
        ) else {
            return;
        };
        let mut spec = TransferSpec {
            debit: interledger,
            credit: settlement,
            amount,
            pending: true,
            reference: Reference::Settlement {
                scheme: at.scheme,
                id: at.scheme,
            },
        };
        // Checked before the scheme is asked, so a refusal leaves nothing
        // prepared behind; with the journal taken, so it has room.
        self.take_journal();
        if let Err(error) = self.ledger.validate(&spec) {
            self.report(SettlementEvent::BooksCannotSettle { node, error });
            return;
        }
        let Some(prepared) = self.scheme_prepare(node, at.scheme, amount, state) else {
            return;
        };
        assert!(
            prepared.amount > 0 && prepared.amount <= amount,
            "a scheme settles at most what was asked"
        );
        assert_eq!(
            self.whole_units(&at.scheme, prepared.amount),
            prepared.amount,
            "a scheme settles in whole units of its ledger"
        );
        spec.amount = prepared.amount;
        spec.reference = Reference::Settlement {
            scheme: at.scheme,
            id: prepared.id,
        };
        // The books took it for at least this amount just now, and nothing
        // was booked since.
        let transfer = match self.ledger.create(self.now_ms, &spec) {
            Ok(transfer) => transfer,
            Err(error) => {
                panic!("a settlement's transfer was refused after it checked out: {error}")
            }
        };
        let amount = prepared.amount;
        self.report(SettlementEvent::Settling {
            node,
            amount,
            id: prepared.id,
        });
        self.settle.settling.insert(
            (at.scheme, prepared.id),
            Settling {
                peer: node,
                transfer,
                amount,
                message: prepared.message,
            },
        );
        self.settle.to_execute.push((at.scheme, prepared.id));
        self.take_journal();
        // What the scheme signed is kept with its pending transfer.
        self.checkpoint(at.scheme);
    }

    /// Asks `scheme` to prepare a settlement of `amount` to `node`.
    fn scheme_prepare(
        &mut self,
        node: NodeId,
        scheme: Name,
        amount: u64,
        state: &[u8],
    ) -> Option<dassie_settlement::PreparedSettlement> {
        let name = Name::new(node.as_str()).ok()?;
        let now = self.now_ms;
        if let Some(asset) = self.schemes.get(&scheme).and_then(|active| active.asset) {
            self.tell_owner_funds(asset);
        }
        let Self {
            schemes,
            assets,
            rng,
            settle,
            events,
            ..
        } = self;
        let active = schemes.get_mut(&scheme).filter(|a| a.failed.is_none())?;
        let asset = match active.asset {
            Some(id) => Some(assets.get_mut(&id)?),
            None => None,
        };
        let asset_status = asset.as_ref().map(|asset| asset.asset.status());
        let prepared = active.scheme.prepare_settlement(
            rng.as_mut(),
            asset.map(|asset| asset.asset.as_mut()),
            name,
            amount,
            state,
        );
        match prepared {
            Ok(prepared) => Some(prepared),
            // Not now: asked again at the next check, with no backoff, so
            // a slow ledger does not hold our peers' liquidity for long.
            // Said now and then, for the owner.
            Err(dassie_settlement::SchemeError::Unavailable) => {
                let warned = settle.unavailable_warned.entry(scheme).or_insert(0);
                if *warned == 0 || now.saturating_sub(*warned) >= UNAVAILABLE_WARN_MS {
                    *warned = now.max(1);
                    let status =
                        super::schemes::merged_status(asset_status, active.scheme.status());
                    events.push(
                        SettlementEvent::SchemeUnavailable {
                            node,
                            scheme,
                            status,
                        }
                        .into(),
                    );
                }
                None
            }
            Err(error) => {
                events.push(SettlementEvent::PrepareFailed { node, error }.into());
                None
            }
        }
    }

    /// Executes the settlements whose pending transfers are now on disk.
    /// Returns whether there were any, whose outcome is to be persisted.
    pub(super) fn execute_settlements(&mut self) -> bool {
        if self.settle.to_execute.is_empty() {
            return false;
        }
        for (scheme, id) in std::mem::take(&mut self.settle.to_execute) {
            // A scheme removed or stopped since stays pending: its next
            // activation reconciles it.
            let Some((active, asset)) = self.scheme_and_asset(scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.execute_settlement(asset, id, &mut effects);
            let ledger = active.scheme.ledger();
            self.scheme_effects(scheme, ledger, &effects);
        }
        true
    }

    /// Hands each scheme its settlements still pending from before a
    /// restart.
    /// Only `scheme`'s if one is named; settlements in progress in this
    /// run are not the scheme's to reconcile.
    pub(super) fn reconcile_settlements(&mut self, only: Option<Name>) {
        let schemes: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(scheme, active)| {
                active.failed.is_none() && only.is_none_or(|only| only == **scheme)
            })
            .map(|(scheme, _)| *scheme)
            .collect();
        for scheme in schemes {
            let pending: Vec<(Name, u64)> = self
                .ledger
                .pending_transfers()
                .filter_map(|transfer| match transfer.reference {
                    Reference::Settlement { scheme: s, id }
                        if s == scheme && !self.settle.settling.contains_key(&(s, id)) =>
                    {
                        Some((id, transfer.amount))
                    }
                    _ => None,
                })
                .collect();
            if pending.is_empty() {
                continue;
            }
            let Some((active, asset)) = self.scheme_and_asset(scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.reconcile(asset, &pending, &mut effects);
            let ledger = active.scheme.ledger();
            self.report(SettlementEvent::Reconciling {
                scheme,
                pending: pending.len(),
            });
            self.scheme_effects(scheme, ledger, &effects);
        }
    }

    /// One of our settlements, by scheme and ID: in progress, or pending in
    /// the ledger from before a restart.
    pub(super) fn take_settling(&mut self, scheme: Name, id: Name) -> Option<Settling> {
        if let Some(settling) = self.settle.settling.remove(&(scheme, id)) {
            return Some(settling);
        }
        let transfer = self
            .ledger
            .pending_transfers()
            .find(|transfer| transfer.reference == Reference::Settlement { scheme, id })?;
        let path = self.ledger.account(transfer.debit)?.path();
        let AccountName::AssetsInterledger(peer) = path.name else {
            return None;
        };
        // The scheme makes its message again from the settlement's ID.
        let message = self
            .schemes
            .get(&scheme)
            .map(|active| active.scheme.settlement_message(id))
            .unwrap_or_default();
        Some(Settling {
            peer: NodeId::parse(peer.as_str()).ok()?,
            transfer: transfer.id,
            amount: transfer.amount,
            message,
        })
    }

    /// Our settlement executed: its transfer is posted, and the peer is
    /// told, until it has heard; both in one batch. If an earlier message
    /// to the peer is still on its way, both wait until it arrives.
    // Panics only on a broken invariant: a settling transfer is pending.
    #[allow(clippy::panic)]
    pub(super) fn settlement_done(&mut self, scheme: Name, id: Name) {
        let Some(settling) = self.take_settling(scheme, id) else {
            self.report(SettlementEvent::UnknownExecuted { scheme, id });
            return;
        };
        if self.peer.settling(&settling.peer) {
            self.settle.settling.insert((scheme, id), settling);
            if !self.settle.announce.contains(&(scheme, id)) {
                self.settle.announce.push((scheme, id));
            }
            return;
        }
        // Settlements in flight are pending transfers, and the journal has
        // room once taken.
        self.check_settling_transfer(scheme, id, &settling);
        self.take_journal();
        if let Err(error) = self.ledger.post(self.now_ms, settling.transfer) {
            panic!("a settlement's transfer could not be posted: {error}");
        }
        let Some(scheme_id) = SchemeId::parse(scheme.as_str()) else {
            return;
        };
        self.settle.backoff.remove(&(settling.peer, scheme_id));
        let peer = settling.peer;
        // A bare notice states no amount: the peer's ledger says what
        // arrived.
        let books = self
            .schemes
            .get(&scheme)
            .is_none_or(|active| active.scheme.books_messages());
        let stated = if books { settling.amount } else { 0 };
        if let Err(error) =
            self.peer
                .send_settlement(self.now_ms, &peer, scheme_id, stated, settling.message)
        {
            self.report(SettlementEvent::AnnounceFailed { peer, error });
        }
    }

    /// Posts and announces the settlements that waited for an earlier
    /// message to arrive.
    fn announce_deferred(&mut self) {
        for (scheme, id) in std::mem::take(&mut self.settle.announce) {
            self.settlement_done(scheme, id);
        }
    }

    /// Our settlement failed for good: its transfer is voided, and the
    /// peer is not settled with again on that method for a while.
    // Panics only on a broken invariant: a settling transfer is pending.
    #[allow(clippy::panic)]
    pub(super) fn settlement_cancelled(&mut self, scheme: Name, id: Name) {
        let Some(settling) = self.take_settling(scheme, id) else {
            self.report(SettlementEvent::UnknownCancelled { scheme, id });
            return;
        };
        self.check_settling_transfer(scheme, id, &settling);
        self.take_journal();
        if let Err(error) = self.ledger.void(self.now_ms, settling.transfer) {
            panic!("a settlement's transfer could not be voided: {error}");
        }
        let now = self.now_ms;
        if let Some(scheme_id) = SchemeId::parse(scheme.as_str()) {
            let key = (settling.peer, scheme_id);
            let (_, failures) = self.settle.backoff.get(&key).copied().unwrap_or((0, 0));
            let wait = CANCEL_BACKOFF_MIN_MS
                .saturating_mul(1u64.checked_shl(failures.min(32)).unwrap_or(u64::MAX))
                .min(CANCEL_BACKOFF_MAX_MS);
            self.settle
                .backoff
                .insert(key, (now.saturating_add(wait), failures.saturating_add(1)));
        }
        self.report(SettlementEvent::Failed {
            peer: settling.peer,
            amount: settling.amount,
        });
    }

    /// A peer's settlement: booked first, so the scheme's balance never
    /// moves without the books. Returns whether it was applied.
    pub(super) fn incoming_settlement(
        &mut self,
        from: NodeId,
        scheme: SchemeId,
        amount: u64,
        data: &[u8],
    ) -> Result<(), EngineError> {
        let refused = |why: &str| EngineError::Invalid(why.into());
        let scheme = Name::new(scheme.as_str()).map_err(|_| refused("unknown scheme"))?;
        let active = self
            .schemes
            .get(&scheme)
            .ok_or_else(|| refused("the scheme is not active"))?;
        if active.failed.is_some() {
            return Err(refused("the scheme has stopped"));
        }
        let ledger = active.scheme.ledger();
        // A bare notice books nothing, so there is nothing to check its
        // amount against.
        let books = active.scheme.books_messages();
        let name = Name::new(from.as_str()).map_err(|_| refused("not a node"))?;
        let interledger = self.ledger.find(&AccountPath::new(
            ledger,
            AccountName::AssetsInterledger(name),
        ));
        if books {
            match interledger {
                Some(interledger) => {
                    let spec = TransferSpec {
                        debit: self.account(ledger, AccountName::AssetsSettlement)?,
                        credit: interledger,
                        amount,
                        pending: false,
                        reference: Reference::Settlement { scheme, id: scheme },
                    };
                    self.ledger
                        .validate(&spec)
                        .map_err(|error| refused(&error.to_string()))?;
                }
                // A downlink nothing moved with yet: its settlement opens its
                // accounts, if there is room for them, and the books take it as
                // they would on accounts just opened
                // (spec/decisions/lazy-downlink-accounts.md).
                None if self.downlink_may_open(&from) => {
                    let debit = self.account(ledger, AccountName::AssetsSettlement)?;
                    let trust =
                        super::downlinks::start_credit(&self.schemes, scheme, self.senior_credit);
                    let reference = Reference::Settlement { scheme, id: scheme };
                    self.ledger
                        .validate_to_peer(debit, name, trust, amount, reference)
                        .map_err(|error| refused(&error.to_string()))?;
                }
                None => return Err(refused("no accounts on that scheme's ledger")),
            }
        }
        // What the scheme keeps for the peer on its method on this scheme;
        // none from a peer without one, as a channel outlives its peering.
        let state = self
            .peer
            .peering_of(&from)
            .and_then(|peering| {
                peering
                    .methods
                    .iter()
                    .find(|method| method.scheme.as_str() == scheme.as_str())
            })
            .map(|method| method.state.clone())
            .unwrap_or_default();
        let mut effects = Vec::new();
        self.schemes
            .get_mut(&scheme)
            .ok_or_else(|| refused("the scheme is not active"))?
            .scheme
            .handle_settlement(name, amount, data, &state, &mut effects)
            .map_err(|error| refused(&error.to_string()))?;
        self.report(SettlementEvent::PeerSettled { from, amount });
        self.apply_effects(scheme, ledger, &effects)
    }

    /// A reference for an incoming settlement: unique, as the next
    /// transfer's ID is.
    pub(super) fn incoming_reference(&self, scheme: Name) -> Reference {
        let id = Name::new(&format!("in{}", self.ledger.next_transfer_id().0)).unwrap_or(scheme);
        Reference::Settlement { scheme, id }
    }
}

/// Where a payer over a hub stands when asked to settle: as a junior that
/// extends the default credit `credit` would, with the hold it keeps toward
/// the peer counted as what it paid in advance. So it funds the hold with
/// what it would otherwise have paid
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub), whatever the
/// hold's part of the credit it extends.
fn hub_position(at: &Position, credit: u64) -> u128 {
    let position = i128::try_from(at.position).unwrap_or(i128::MAX);
    let standing = position - i128::from(at.credit_out) + i128::from(credit);
    u128::try_from(standing).unwrap_or(0)
}

/// Whether `transfer`, in `ledger`, is a settlement of ours to `node`.
pub(super) fn settles_to(
    ledger: &dassie_ledger::Ledger,
    transfer: &Transfer,
    node: &NodeId,
) -> bool {
    matches!(transfer.reference, Reference::Settlement { .. })
        && ledger.account(transfer.debit).is_some_and(|account| {
            matches!(account.path().name,
                AccountName::AssetsInterledger(peer) if peer.as_str() == node.as_str())
        })
}
