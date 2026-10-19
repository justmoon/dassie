//! Settling with peers (D23,
//! spec/areas/settlement.md#asking-a-peer-to-settle): a node asks a peer to
//! settle once the peer is close to the credit it extends, and settles half
//! the way to its own trigger when a peer asks it. A settlement is prepared,
//! booked as a durable pending transfer, executed once that is on disk, and
//! announced to the peer once it executed. Incoming settlements are booked
//! before the scheme sees them.

use std::collections::BTreeMap;

use dassie_identity::NodeId;
use dassie_ledger::{
    AccountName, AccountPath, LedgerId, Name, Reference, Transfer, TransferId, TransferSpec,
};
use dassie_peer::Room;
use dassie_peer::wire::SchemeId;
use dassie_settlement::{LedgerStatus, policy};

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
}

/// The settling side of the core.
pub(super) struct Settlements {
    /// When what is owed to peers is checked next.
    pub at_ms: u64,
    pub interval_ms: u64,
    /// How often a peer that did not settle since is asked again, at most.
    pub request_every_ms: u64,
    /// Our trigger for every peer, and the least we settle, from the
    /// largest packet (D25).
    pub trigger: u64,
    pub minimum: u64,
    /// The most credit a peer can earn (D20).
    pub credit_cap: u64,
    /// Where reading the fees peers earned us stands.
    pub growth: super::growth::Growth,
    /// When each peer was last asked to settle, until it settles.
    pub asked: BTreeMap<NodeId, u64>,
    /// Our settlements prepared and not yet final, by scheme and ID.
    pub settling: BTreeMap<(Name, Name), Settling>,
    /// Settlements to execute once their pending transfers are on disk.
    pub to_execute: Vec<(Name, Name)>,
    /// Peers not settled with before a time, after cancelled settlements,
    /// and how many in a row.
    pub backoff: BTreeMap<NodeId, (u64, u32)>,
    /// Settlements that executed while an earlier one to the same peer was
    /// still being announced: posted and announced once that one arrives.
    pub announce: Vec<(Name, Name)>,
    /// When each scheme last said it cannot settle.
    pub unavailable_warned: BTreeMap<Name, u64>,
    /// Whether this node lies that it never settles, in a simulation.
    pub never_settles: bool,
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
            growth: super::growth::Growth::new(),
            asked: BTreeMap::new(),
            settling: BTreeMap::new(),
            to_execute: Vec::new(),
            backoff: BTreeMap::new(),
            announce: Vec::new(),
            unavailable_warned: BTreeMap::new(),
            never_settles: false,
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
        }
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
    /// waiting for a routine settlement (D12). Each settlement booked from
    /// it starts the grace again. While our scheme does not see its ledger,
    /// we cannot ask: nothing is judged, and the time short neither grows
    /// nor starts again.
    fn report_room(&mut self) {
        let now = self.now_ms;
        let peers: Vec<(NodeId, SchemeId)> = self
            .peer
            .active_peers()
            .map(|(id, peering)| (*id, peering.scheme))
            .collect();
        for (node, scheme) in peers {
            let room = if !self.sees_ledger(scheme) {
                Room::Unjudged
            } else if self
                .position(node, scheme)
                // `minimum` is one of the largest packets (D25).
                .is_some_and(|at| at.position < u128::from(self.settle.minimum))
            {
                // Two checks' worth at most: more is a stall of ours.
                Room::Short {
                    most_ms: self.settle.interval_ms.saturating_mul(2),
                }
            } else {
                Room::Enough
            };
            self.peer.room(now, &node, room);
        }
    }

    /// Whether `scheme` sees its ledger, so that a peer on it can be asked
    /// to settle.
    fn sees_ledger(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str()).ok().is_some_and(|name| {
            self.schemes
                .get(&name)
                .is_some_and(|active| active.scheme.status() == LedgerStatus::Synced)
        })
    }

    /// Asks each peer that is close to the credit we extend to settle:
    /// again at the next check once our books show a settlement from it,
    /// and otherwise at most once per [`Settlements::request_every_ms`],
    /// for a peer that does not answer. Only our own books decide; the peer
    /// decides how much (D23).
    ///
    /// A peer is asked only while our scheme sees its ledger. An honest
    /// peer settles only when asked, so none of its settlements reaches the
    /// ledger before our scheme first synced, which books what the account
    /// held by then as the owner's deposit.
    fn ask_peers(&mut self) {
        let now = self.now_ms;
        let every = self.settle.request_every_ms;
        let peers: Vec<(NodeId, SchemeId)> = self
            .peer
            .active_peers()
            .map(|(id, peering)| (*id, peering.scheme))
            .collect();
        self.settle
            .asked
            .retain(|node, _| peers.iter().any(|(peer, _)| peer == node));
        for (node, scheme) in peers {
            if !self.sees_ledger(scheme) {
                continue;
            }
            let due = self
                .settle
                .asked
                .get(&node)
                .is_none_or(|at| at.saturating_add(every) <= now);
            let close = self
                .position(node, scheme)
                .is_some_and(|at| policy::ask(at.position, self.settle.trigger));
            if due && close && self.peer.request_settlement(now, &node, scheme).is_ok() {
                self.settle.asked.insert(node, now);
            }
        }
        self.after_peer();
    }

    /// A peer asks us to settle on `scheme`: we settle half the way to our
    /// own trigger for it, unless a settlement to it is on its way already,
    /// which is the answer (D23).
    ///
    /// Returns what is then on its way to the peer, which the answer tells
    /// it (D28): a settlement already under way, which is the whole answer,
    /// or the one made now.
    pub(super) fn settlement_requested(&mut self, node: NodeId, scheme: SchemeId) -> u64 {
        if self.settle.never_settles {
            return 0;
        }
        let under_way = self.settling_to(&node);
        if under_way > 0 || !self.can_settle(&node) {
            return under_way;
        }
        let Some(state) = self
            .peer
            .peering_of(&node)
            .filter(|peering| peering.scheme == scheme)
            .map(|peering| peering.state.clone())
        else {
            return 0;
        };
        let Some(at) = self.position(node, scheme) else {
            return 0;
        };
        let amount = self.whole_units(
            &at.scheme,
            policy::requested_amount(at.position, self.settle.trigger, self.settle.minimum),
        );
        if amount > 0 {
            self.prepare_settlement(node, &at, amount, &state);
        }
        self.settling_to(&node)
    }

    /// What is on its way to `node`, in its scheme ledger's units: our
    /// settlements still pending in the ledger (prepared, executing, or
    /// waiting to be announced), and the one announced that it has not yet
    /// acknowledged.
    fn settling_to(&self, node: &NodeId) -> u64 {
        let pending: u64 = self
            .ledger
            .pending_transfers()
            .filter(|transfer| self.settles_to(transfer, node))
            .map(|transfer| transfer.amount)
            .sum();
        let announced = self
            .peer
            .channel(node)
            .and_then(|channel| channel.outbox.as_ref())
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

    /// Whether nothing is on its way to `node`: no message, no settlement
    /// of ours in progress or still pending from before a restart; that we
    /// hold a channel with it; and that no cancelled settlement holds it
    /// off (ACC-17).
    fn can_settle(&self, node: &NodeId) -> bool {
        self.peer.channel(node).is_some()
            && !self.peer.settling(node)
            && !self
                .settle
                .settling
                .values()
                .any(|settling| settling.peer == *node)
            && !self.pending_settlement_to(node)
            && self
                .settle
                .backoff
                .get(node)
                .is_none_or(|(until, _)| *until <= self.now_ms)
    }

    /// Whether a settlement transfer to `node` is pending in the ledger,
    /// on any scheme.
    fn pending_settlement_to(&self, node: &NodeId) -> bool {
        self.ledger
            .pending_transfers()
            .any(|transfer| self.settles_to(transfer, node))
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
        ))?;
        let trust = self
            .ledger
            .find(&AccountPath::new(ledger, AccountName::ContraTrust(name)))?;
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
    /// once that is on disk (ACC-16).
    // Panics only on a broken invariant: what was validated is taken.
    #[allow(clippy::panic)]
    fn prepare_settlement(&mut self, node: NodeId, at: &Position, amount: u64, state: &[u8]) {
        let Ok(name) = Name::new(node.as_str()) else {
            return;
        };
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
        if let Some(state) = prepared.peer_state {
            self.peer.set_scheme_state(&node, state);
        }
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
        let Self {
            schemes,
            rng,
            settle,
            events,
            ..
        } = self;
        let active = schemes.get_mut(&scheme).filter(|a| a.failed.is_none())?;
        match active
            .scheme
            .prepare_settlement(rng.as_mut(), name, amount, state)
        {
            Ok(prepared) => Some(prepared),
            // Not now: asked again at the next check, with no backoff, so
            // a slow ledger does not hold our peers' liquidity for long.
            // Said now and then, for the owner.
            Err(dassie_settlement::SchemeError::Unavailable) => {
                let warned = settle.unavailable_warned.entry(scheme).or_insert(0);
                if *warned == 0 || now.saturating_sub(*warned) >= UNAVAILABLE_WARN_MS {
                    *warned = now.max(1);
                    let status = active.scheme.status();
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
            // A scheme removed since stays pending: its next activation
            // reconciles it.
            let Some(active) = self.schemes.get_mut(&scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.execute_settlement(id, &mut effects);
            let ledger = active.scheme.ledger();
            if let Err(error) = self.apply_effects(scheme, ledger, &effects) {
                self.report(SettlementEvent::OutcomeNotBooked { error });
            }
        }
        true
    }

    /// Hands each scheme its settlements still pending from before a
    /// restart (ACC-16).
    /// Only `scheme`'s if one is named; settlements in progress in this
    /// run are not the scheme's to reconcile.
    pub(super) fn reconcile_settlements(&mut self, only: Option<Name>) {
        let schemes: Vec<Name> = self
            .schemes
            .keys()
            .copied()
            .filter(|scheme| only.is_none_or(|only| only == *scheme))
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
            let Some(active) = self.schemes.get_mut(&scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.reconcile(&pending, &mut effects);
            let ledger = active.scheme.ledger();
            self.report(SettlementEvent::Reconciling {
                scheme,
                pending: pending.len(),
            });
            if let Err(error) = self.apply_effects(scheme, ledger, &effects) {
                self.report(SettlementEvent::ReconciliationNotBooked { error });
            }
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
        self.settle.backoff.remove(&settling.peer);
        let Some(scheme_id) = SchemeId::parse(scheme.as_str()) else {
            return;
        };
        let peer = settling.peer;
        if let Err(error) = self.peer.send_settlement(
            self.now_ms,
            &peer,
            scheme_id,
            settling.amount,
            settling.message,
        ) {
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
    /// peer is not settled with again for a while.
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
        let (_, failures) = self
            .settle
            .backoff
            .get(&settling.peer)
            .copied()
            .unwrap_or((0, 0));
        let wait = CANCEL_BACKOFF_MIN_MS
            .saturating_mul(1u64.checked_shl(failures.min(32)).unwrap_or(u64::MAX))
            .min(CANCEL_BACKOFF_MAX_MS);
        self.settle.backoff.insert(
            settling.peer,
            (now.saturating_add(wait), failures.saturating_add(1)),
        );
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
        let ledger = self
            .schemes
            .get(&scheme)
            .map(|active| active.scheme.ledger())
            .ok_or_else(|| refused("the scheme is not active"))?;
        let name = Name::new(from.as_str()).map_err(|_| refused("not a node"))?;
        let interledger = self
            .ledger
            .find(&AccountPath::new(
                ledger,
                AccountName::AssetsInterledger(name),
            ))
            .ok_or_else(|| refused("no accounts on that scheme's ledger"))?;
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
        let state = self
            .peer
            .peering_of(&from)
            .map(|peering| peering.state.clone())
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

    /// A scheme's message from a peer, for the scheme.
    pub(super) fn scheme_message(&mut self, from: NodeId, scheme: SchemeId, message: &[u8]) {
        let (Ok(scheme), Ok(name)) = (Name::new(scheme.as_str()), Name::new(from.as_str())) else {
            return;
        };
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        let mut effects = Vec::new();
        active.scheme.handle_message(name, message, &mut effects);
        let ledger = active.scheme.ledger();
        if let Err(error) = self.apply_effects(scheme, ledger, &effects) {
            self.report(SettlementEvent::MessageEffectsNotBooked { error });
        }
    }

    /// A reference for an incoming settlement: unique, as the next
    /// transfer's ID is.
    pub(super) fn incoming_reference(&self, scheme: Name) -> Reference {
        let id = Name::new(&format!("in{}", self.ledger.next_transfer_id().0)).unwrap_or(scheme);
        Reference::Settlement { scheme, id }
    }
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
