//! One grace limit per peer (D18): what a peer cost us, read from the
//! books, against one limit; past it, the peer is banned and the owner is
//! told.
//!
//! - **Refused:** the losses its refused Fulfills booked (D17) in the
//!   window, or since the owner last lifted a ban on it. Refusals our own
//!   receiver took back cost nothing and do not count.
//! - **Overdue:** what it owes us, posted, once that has stayed past the
//!   point where it should have settled for longer than the grace for
//!   settling, while our scheme could see its settlements. That point is
//!   its own settlement threshold, at the default ratio or ours if higher,
//!   and never below half the credit we extend. Only while peered: a
//!   peering being ended cannot settle until it is over, and one that
//!   ended without the peer hearing of it never settles at all; its debt
//!   stays on the books.
//!
//! The limit is a share of the credit we extend, with a floor. A peer can
//! cost us at most its limit plus what it has in flight before its ban:
//! the credit both ways.

use std::collections::{BTreeMap, BTreeSet};

use dassie_identity::NodeId;
use dassie_ledger::{AccountName, AccountPath, LedgerId, Name};
use dassie_peer::BanError;
use dassie_peer::wire::{SchemeId, TerminationReason};
use dassie_settlement::policy;

use super::Core;
use super::events::{PeerEvent, StoreRead};
use crate::store::{BanChange, BanReason, BanRecord, MAX_BAN_RECORDS, PeerCost, PeerSettings};

/// The grace limit's timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GracePolicy {
    /// How far back refusals count.
    pub window_ms: u64,
    /// How long a peer may owe us more than half its credit.
    pub settle_grace_ms: u64,
    /// How often every peer is checked; one whose refusal was just booked
    /// is checked at once.
    pub check_every_ms: u64,
}

impl GracePolicy {
    /// The production values.
    pub const DEFAULT: Self = Self {
        window_ms: 30 * 24 * 60 * 60 * 1_000,
        settle_grace_ms: 60 * 60 * 1_000,
        check_every_ms: 60 * 1_000,
    };

    /// Shorter waits, for dev environments and tests: still longer than a
    /// settlement on the XRP Ledger takes to be given up on, once its
    /// submission is lost (20 ledgers), and tried again.
    pub const DEV: Self = Self {
        settle_grace_ms: 5 * 60 * 1_000,
        check_every_ms: 5 * 1_000,
        ..Self::DEFAULT
    };
}

/// A lie a node tells its peers, in a simulation of a dishonest network:
/// its own code stays honest otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lie {
    /// It takes Fulfills and says it refused them.
    RefuseFulfills,
    /// It never settles what it owes.
    NeverSettle,
}

/// How a peer stands with us, for what counts against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    Peered,
    Terminating,
    Ended,
}

/// The grace limits' state: all of it rebuilt from the books but the
/// clocks of what is due, which start no earlier than our own start.
pub(super) struct Grace {
    policy: GracePolicy,
    /// When every peer is checked next.
    at_ms: u64,
    /// Peers to check at the end of this step.
    queued: BTreeSet<NodeId>,
    /// Since when each peer has owed us past the point where it should
    /// have settled.
    due_since: BTreeMap<NodeId, u64>,
    /// The schemes that see their ledgers, as of the last check.
    synced: BTreeSet<Name>,
    /// Peers past their limit whose peering was ended without a ban, the
    /// ban list being full: not ended again at each check.
    over_limit: BTreeSet<NodeId>,
    /// The settlement ratio a peer is held to: the default or ours,
    /// whichever is higher, so a peer that settles later than we do is
    /// not overdue for it.
    ratio_ppm: u32,
    /// Each peer's cost at its last check, for the owner's view.
    costs: BTreeMap<NodeId, PeerCost>,
    /// The lies this node tells, in a simulation.
    lies: BTreeSet<Lie>,
}

impl Grace {
    pub(super) fn new(now_ms: u64, policy: GracePolicy, ratio_ppm: u32) -> Self {
        Self {
            policy,
            at_ms: now_ms.saturating_add(policy.check_every_ms),
            queued: BTreeSet::new(),
            due_since: BTreeMap::new(),
            synced: BTreeSet::new(),
            over_limit: BTreeSet::new(),
            ratio_ppm: ratio_ppm.max(policy::DEFAULT_RATIO_PPM),
            costs: BTreeMap::new(),
            lies: BTreeSet::new(),
        }
    }

    /// When every peer is checked next.
    pub(super) const fn at_ms(&self) -> u64 {
        self.at_ms
    }

    /// Whether peers wait to be checked.
    pub(super) fn has_queued(&self) -> bool {
        !self.queued.is_empty()
    }

    /// Checks `peer` at the end of this step: a refusal was booked.
    pub(super) fn check_soon(&mut self, peer: NodeId) {
        self.queued.insert(peer);
    }

    /// What `peer` cost us at its last check.
    pub(super) fn cost(&self, peer: &NodeId) -> Option<PeerCost> {
        self.costs.get(peer).copied()
    }
}

/// The least a grace limit is: a hundredth of a unit, so a peer we extend
/// no credit to is not banned for its first late Fulfill.
const fn grace_floor(ledger: LedgerId) -> u64 {
    let scale = ledger.asset().scale;
    if scale < 2 {
        1
    } else {
        10u64.pow((scale - 2) as u32)
    }
}

/// `credit_out` times the owner's share, at least the floor, and within
/// what the ledger holds.
fn grace_limit(ledger: LedgerId, credit_out: u64, share_ppm: u32) -> u64 {
    let share = u128::from(credit_out) * u128::from(share_ppm) / 1_000_000;
    u64::try_from(share)
        .unwrap_or(u64::MAX)
        .min(dassie_ledger::MAX_TOTAL)
        .max(grace_floor(ledger))
}

/// What a peer may owe us before it should have settled: past its own
/// threshold at `ratio_ppm` (spec 5 §4.6, with the credits as it sees
/// them), and never below half the credit we extend.
fn due_point(credit_in: u64, credit_out: u64, ratio_ppm: u32) -> u64 {
    let middle = (u128::from(credit_in) + u128::from(credit_out)) / 2;
    let threshold = middle + middle * u128::from(ratio_ppm) / 2_000_000;
    let owes = threshold.saturating_sub(u128::from(credit_in));
    u64::try_from(owes).unwrap_or(u64::MAX).max(credit_out / 2)
}

/// A peer's debt to us on a ledger: what it owes, posted, and the credit
/// we extend.
struct Debt {
    ledger: LedgerId,
    interledger: dassie_ledger::AccountId,
    owes: u64,
    credit_out: u64,
}

impl Core {
    /// Makes this node tell `lie`, in a simulation of a dishonest network.
    #[cfg(feature = "testing")]
    pub fn lie(&mut self, lie: Lie) {
        self.grace.lies.insert(lie);
    }

    /// Whether this node tells `lie`.
    pub(super) fn lies(&self, lie: Lie) -> bool {
        self.grace.lies.contains(&lie)
    }

    /// The owner's peer settings.
    pub const fn peer_settings(&self) -> PeerSettings {
        self.peer_settings
    }

    /// Changes the owner's peer settings; the next check applies them.
    ///
    /// # Errors
    ///
    /// They are out of range.
    pub fn set_peer_settings(&mut self, settings: PeerSettings) -> Result<(), super::EngineError> {
        if !settings.is_valid() {
            return Err(super::EngineError::Invalid(
                "the grace share must be more than 0 and at most 10 000 000 ppm, \
                 the fee and the spread at most 100 000 ppm"
                    .into(),
            ));
        }
        self.connector.set_charges(settings.charges());
        self.peer_settings = settings;
        self.batch.peer_settings = Some(settings);
        self.grace.at_ms = self.now_ms;
        self.changes.network = true;
        Ok(())
    }

    /// Checks what peers cost us, after this step's books are on disk:
    /// every peer when it is time, and those whose refusals were just
    /// booked. Bans any past its limit; returns whether that changed the
    /// peering, which is for the disk in turn.
    pub(super) fn grace_checks(&mut self) -> bool {
        let now = self.now_ms;
        let periodic = self.grace.at_ms <= now;
        if !periodic && self.grace.queued.is_empty() {
            return false;
        }
        let subjects: Vec<NodeId> = if periodic {
            self.grace.at_ms = now.saturating_add(self.grace.policy.check_every_ms);
            self.grace.queued.clear();
            self.update_synced();
            self.peer.channels().map(|(id, _)| *id).collect()
        } else {
            std::mem::take(&mut self.grace.queued).into_iter().collect()
        };
        let active: BTreeSet<NodeId> = self.peer.active_peers().map(|(id, _)| *id).collect();
        let mut changed = false;
        for node in &subjects {
            let cost = if self.peer.is_banned(node) || self.grace.over_limit.contains(node) {
                None
            } else {
                self.peer_cost(*node, &active)
            };
            let Some(cost) = cost else {
                self.grace.due_since.remove(node);
                self.grace.costs.remove(node);
                continue;
            };
            if cost.over() {
                changed |= self.ban_for(*node, cost);
            } else {
                self.grace.costs.insert(*node, cost);
            }
        }
        if periodic {
            let checked: BTreeSet<NodeId> = subjects.into_iter().collect();
            self.grace
                .due_since
                .retain(|node, _| checked.contains(node));
            self.grace.costs.retain(|node, _| checked.contains(node));
            self.grace.over_limit.retain(|node| checked.contains(node));
        }
        changed
    }

    /// Notes which schemes see their ledgers. A scheme that does not stops
    /// every clock of what is due on its ledger.
    pub(super) fn update_synced(&mut self) {
        let synced: BTreeSet<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.scheme.synced())
            .map(|(name, _)| *name)
            .collect();
        self.grace.synced = synced;
    }

    /// The scheme `node`'s accounts are on and how it stands with us: the
    /// peering's, or for a peering that ended, the first active scheme
    /// whose ledger holds its accounts.
    fn standing(&self, node: &NodeId, active: &BTreeSet<NodeId>) -> Option<(SchemeId, Standing)> {
        if let Some(peering) = self.peer.peering_of(node) {
            let standing = if active.contains(node) {
                Standing::Peered
            } else {
                Standing::Terminating
            };
            return Some((peering.scheme, standing));
        }
        let name = Name::new(node.as_str()).ok()?;
        self.schemes.iter().find_map(|(scheme, active)| {
            self.ledger
                .find(&AccountPath::new(
                    active.scheme.ledger(),
                    AccountName::AssetsInterledger(name),
                ))
                .and_then(|_| SchemeId::parse(scheme.as_str()))
                .map(|scheme| (scheme, Standing::Ended))
        })
    }

    /// What `node` owes us on `scheme`'s ledger, posted: packets in flight
    /// and our own settlements on their way are not debts yet.
    fn debt(&self, node: NodeId, scheme: SchemeId) -> Option<Debt> {
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
        let credit_out = policy::trust(self.ledger.account(trust)?.balance());
        let balance = self.ledger.account(interledger)?.balance();
        let position = balance.credits_posted.saturating_sub(balance.debits_posted);
        let owes = u64::try_from(u128::from(credit_out).saturating_sub(position)).unwrap_or(0);
        Some(Debt {
            ledger,
            interledger,
            owes,
            credit_out,
        })
    }

    /// Whether `node`, peered on `scheme`, owes us past the point where it
    /// should have settled, as `debt` says.
    fn past_due(&self, node: &NodeId, scheme: SchemeId, debt: &Debt) -> bool {
        let Some(peering) = self.peer.peering_of(node) else {
            return false;
        };
        let Ok(name) = Name::new(scheme.as_str()) else {
            return false;
        };
        self.grace.synced.contains(&name)
            && debt.owes > due_point(peering.credit_in, debt.credit_out, self.grace.ratio_ppm)
    }

    /// A settlement from `node` came: if it no longer owes past the point
    /// where it should have settled, the clock of what is due stops now,
    /// not at the next check.
    pub(super) fn grace_settled(&mut self, node: NodeId) {
        let Some(scheme) = self.peer.peering_of(&node).map(|peering| peering.scheme) else {
            return;
        };
        let due = self
            .debt(node, scheme)
            .is_some_and(|debt| self.past_due(&node, scheme, &debt));
        if !due {
            self.grace.due_since.remove(&node);
        }
    }

    /// What `node` cost us, as the books have it now; `None` if it has no
    /// accounts on an active ledger.
    pub(super) fn peer_cost(
        &mut self,
        node: NodeId,
        active: &BTreeSet<NodeId>,
    ) -> Option<PeerCost> {
        let (scheme, standing) = self.standing(&node, active)?;
        let debt = self.debt(node, scheme)?;
        let now = self.now_ms;
        let overdue = if standing == Standing::Peered && self.past_due(&node, scheme, &debt) {
            let since = *self.grace.due_since.entry(node).or_insert(now);
            if now.saturating_sub(since) >= self.grace.policy.settle_grace_ms {
                debt.owes
            } else {
                0
            }
        } else {
            self.grace.due_since.remove(&node);
            0
        };

        let losses = self
            .ledger
            .find(&AccountPath::new(debt.ledger, AccountName::ExpensesLosses));
        let refused = match losses {
            Some(losses) => {
                let window = now.saturating_sub(self.grace.policy.window_ms);
                let since = self
                    .ban_records
                    .get(&node)
                    .and_then(|record| record.unbanned_at_ms)
                    .map_or(window, |lifted| lifted.max(window));
                self.store
                    .losses_since(debt.interledger, losses, since)
                    .unwrap_or_else(|error| {
                        let error = error.to_string();
                        let what = StoreRead::PeerLosses;
                        self.events
                            .push(PeerEvent::StoreReadFailed { what, error }.into());
                        0
                    })
            }
            None => 0,
        };
        Some(PeerCost {
            ledger: debt.ledger,
            refused,
            overdue,
            grace: grace_limit(
                debt.ledger,
                debt.credit_out,
                self.peer_settings.grace_share_ppm,
            ),
        })
    }

    /// Bans `node`, which cost us more than its limit, and keeps why. A
    /// full ban list does not save it: its peering ends all the same, once.
    /// Returns whether the peering changed.
    pub(super) fn ban_for(&mut self, node: NodeId, cost: PeerCost) -> bool {
        let now = self.now_ms;
        self.report(PeerEvent::Banned {
            node,
            ledger: cost.ledger,
            refused: cost.refused,
            overdue: cost.overdue,
            grace: cost.grace,
        });
        self.grace.costs.remove(&node);
        self.grace.due_since.remove(&node);
        match self.peer.ban(now, node) {
            Ok(()) => {}
            Err(BanError::Full) => {
                self.report(PeerEvent::BanListFull { node });
                self.grace.over_limit.insert(node);
                let ended = self.peer.terminate(now, &node, TerminationReason::Owner);
                self.after_peer();
                return ended;
            }
            Err(BanError::Ourselves) => return false,
        }
        self.record_ban(BanRecord {
            node,
            reason: BanReason::GraceLimit,
            banned_at_ms: now,
            cost: Some(cost),
            unbanned_at_ms: None,
        });
        self.after_peer();
        true
    }

    /// Keeps why `record.node` was banned, within [`MAX_BAN_RECORDS`]: the
    /// records of the bans lifted longest ago go first.
    pub(super) fn record_ban(&mut self, record: BanRecord) {
        self.ban_records.insert(record.node, record);
        self.batch.bans.push(BanChange::Banned(record));
        while self.ban_records.len() > MAX_BAN_RECORDS {
            let oldest = self
                .ban_records
                .values()
                .filter_map(|record| record.unbanned_at_ms.map(|at| (at, record.node)))
                .min();
            let Some((_, node)) = oldest else {
                break;
            };
            self.ban_records.remove(&node);
            self.batch.bans.push(BanChange::Forgotten(node));
        }
        self.changes.network = true;
    }

    /// Notes that the ban on `node` was lifted, so what its refusals cost
    /// before does not count again. A ban without a record, from before
    /// the node kept them or forgotten since, gets one now.
    pub(super) fn record_unban(&mut self, node: NodeId) {
        let now = self.now_ms;
        if let Some(record) = self.ban_records.get_mut(&node) {
            record.unbanned_at_ms = Some(now.max(record.banned_at_ms));
            self.batch
                .bans
                .push(BanChange::Unbanned { node, at_ms: now });
        } else {
            self.record_ban(BanRecord {
                node,
                reason: BanReason::Owner,
                banned_at_ms: now,
                cost: None,
                unbanned_at_ms: Some(now),
            });
        }
    }

    /// Why each banned node is, as far as is known.
    pub(super) fn ban_record(&self, node: &NodeId) -> Option<BanRecord> {
        self.ban_records.get(node).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_a_share_of_the_credit_with_a_floor() {
        assert_eq!(
            grace_limit(LedgerId::StubUsd, 1_100_000_000, 500_000),
            550_000_000
        );
        assert_eq!(grace_limit(LedgerId::StubUsd, 0, 500_000), 10_000_000);
        // Within what the ledger holds, so it can be stored.
        assert_eq!(
            grace_limit(LedgerId::StubUsd, dassie_ledger::MAX_TOTAL, 10_000_000),
            dassie_ledger::MAX_TOTAL
        );
    }

    #[test]
    fn a_peer_is_due_past_its_own_threshold() {
        let c = 1_100_000_000;
        // Even credit: its threshold is low, so half our credit decides.
        assert_eq!(due_point(c, c, 200_000), c / 2);
        // It extends us nothing: it settles only past 1.1 × half of ours.
        assert_eq!(due_point(0, c, 200_000), 605_000_000);
        // It extends us more: half our credit still decides.
        assert_eq!(due_point(10 * c, c, 200_000), c / 2);
    }

    #[test]
    fn a_cost_is_over_only_past_its_limit() {
        let cost = PeerCost {
            ledger: LedgerId::StubUsd,
            refused: 5,
            overdue: 5,
            grace: 10,
        };
        assert!(!cost.over());
        assert!(PeerCost { refused: 6, ..cost }.over());
    }
}
