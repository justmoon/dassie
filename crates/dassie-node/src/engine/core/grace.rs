//! One grace limit per peer (D18): what a peer cost us, read from the
//! books, against one limit; past it, the peer is banned and the owner is
//! told.
//!
//! - **Refused:** the losses its refused Fulfills booked (D17) in the
//!   window, or since the owner last lifted a ban on it. Refusals our own
//!   receiver took back cost nothing and do not count.
//! - **Overdue:** what it owes us, posted, once that has stayed above half
//!   the credit we extend for longer than the grace for settling, while
//!   our scheme could see its settlements. Not while the peering is being
//!   ended: it cannot settle until it is over.
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
use crate::log::target;
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

    /// Shorter waits, for dev environments and tests.
    pub const DEV: Self = Self {
        settle_grace_ms: 60 * 1_000,
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
    /// Since when each peer has owed us more than half its credit.
    due_since: BTreeMap<NodeId, u64>,
    /// Since when each scheme has seen its ledger.
    synced_since: BTreeMap<Name, u64>,
    /// Each peer's cost at its last check, for the owner's view.
    costs: BTreeMap<NodeId, PeerCost>,
    /// The lies this node tells, in a simulation.
    lies: BTreeSet<Lie>,
}

impl Grace {
    pub(super) const fn new(now_ms: u64, policy: GracePolicy) -> Self {
        Self {
            policy,
            at_ms: now_ms.saturating_add(policy.check_every_ms),
            queued: BTreeSet::new(),
            due_since: BTreeMap::new(),
            synced_since: BTreeMap::new(),
            costs: BTreeMap::new(),
            lies: BTreeSet::new(),
        }
    }

    /// When every peer is checked next.
    pub(super) const fn at_ms(&self) -> u64 {
        self.at_ms
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

/// `credit_out` times the owner's share, at least the floor.
fn grace_limit(ledger: LedgerId, credit_out: u64, share_ppm: u32) -> u64 {
    let share = u128::from(credit_out) * u128::from(share_ppm) / 1_000_000;
    u64::try_from(share)
        .unwrap_or(u64::MAX)
        .max(grace_floor(ledger))
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
                "the grace share must be more than 0 and at most 10 000 000 ppm".into(),
            ));
        }
        self.peer_settings = settings;
        self.batch.peer_settings = Some(settings);
        self.grace.at_ms = self.now_ms;
        self.changes.network = true;
        Ok(())
    }

    /// Checks what peers cost us, after this step's books are on disk:
    /// every peer when it is time, and those whose refusals were just
    /// booked. Bans any past its limit; returns whether it did, which is
    /// for the disk in turn.
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
        let mut banned = false;
        for node in &subjects {
            let cost = if self.peer.is_banned(node) {
                None
            } else {
                self.peer_cost(*node)
            };
            let Some(cost) = cost else {
                self.grace.due_since.remove(node);
                self.grace.costs.remove(node);
                continue;
            };
            if cost.over() {
                self.ban_for(*node, cost);
                banned = true;
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
        }
        banned
    }

    /// Notes which schemes see their ledgers, and since when. A scheme that
    /// does not stops every clock of what is due on its ledger.
    pub(super) fn update_synced(&mut self) {
        let now = self.now_ms;
        let synced: Vec<(Name, bool)> = self
            .schemes
            .iter()
            .map(|(name, active)| (*name, active.scheme.synced()))
            .collect();
        self.grace
            .synced_since
            .retain(|name, _| synced.iter().any(|(s, ok)| s == name && *ok));
        for (name, ok) in synced {
            if ok {
                self.grace.synced_since.entry(name).or_insert(now);
            }
        }
    }

    /// The scheme `node`'s accounts are on and how it stands with us: the
    /// peering's, or for a peering that ended, the first active scheme
    /// whose ledger holds its accounts.
    fn standing(&self, node: &NodeId) -> Option<(SchemeId, Standing)> {
        if let Some(peering) = self.peer.peering_of(node) {
            let active = self.peer.active_peers().any(|(id, _)| id == node);
            let standing = if active {
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

    /// What `node` cost us, as the books have it now; `None` if it has no
    /// accounts on an active ledger.
    pub(super) fn peer_cost(&mut self, node: NodeId) -> Option<PeerCost> {
        let (scheme_id, standing) = self.standing(&node)?;
        let scheme = Name::new(scheme_id.as_str()).ok()?;
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
        // Posted only: packets in flight, and our own settlements on their
        // way, are not debts yet.
        let position = balance.credits_posted.saturating_sub(balance.debits_posted);
        let owes = u64::try_from(u128::from(credit_out).saturating_sub(position)).unwrap_or(0);

        let now = self.now_ms;
        let synced = self.grace.synced_since.contains_key(&scheme);
        let overdue = if standing != Standing::Terminating && synced && owes > credit_out / 2 {
            let since = *self.grace.due_since.entry(node).or_insert(now);
            if now.saturating_sub(since) >= self.grace.policy.settle_grace_ms {
                owes
            } else {
                0
            }
        } else {
            self.grace.due_since.remove(&node);
            0
        };

        let losses = self
            .ledger
            .find(&AccountPath::new(ledger, AccountName::ExpensesLosses));
        let refused = match losses {
            Some(losses) => {
                let window = now.saturating_sub(self.grace.policy.window_ms);
                let since = self
                    .ban_records
                    .get(&node)
                    .and_then(|record| record.unbanned_at_ms)
                    .map_or(window, |lifted| lifted.max(window));
                self.store
                    .losses_since(interledger, losses, since)
                    .unwrap_or_else(|error| {
                        tracing::error!(target: target::PEER, %error, "reading what a peer refused");
                        0
                    })
            }
            None => 0,
        };
        Some(PeerCost {
            ledger,
            refused,
            overdue,
            grace: grace_limit(ledger, credit_out, self.peer_settings.grace_share_ppm),
        })
    }

    /// Bans `node`, which cost us more than its limit, and keeps why. A
    /// full ban list does not save it: its peering ends all the same.
    pub(super) fn ban_for(&mut self, node: NodeId, cost: PeerCost) {
        let now = self.now_ms;
        tracing::warn!(
            target: target::PEER,
            %node,
            ledger = %cost.ledger,
            refused = cost.refused,
            overdue = cost.overdue,
            grace = cost.grace,
            "a peer cost us more than its grace limit: banned",
        );
        match self.peer.ban(now, node) {
            Ok(()) => {}
            Err(BanError::Full) => {
                tracing::error!(target: target::PEER, %node, "the ban list is full: the peering ends, but the node is not banned");
                self.peer.terminate(now, &node, TerminationReason::Owner);
            }
            Err(BanError::Ourselves) => return,
        }
        self.grace.costs.remove(&node);
        self.grace.due_since.remove(&node);
        self.record_ban(BanRecord {
            node,
            reason: BanReason::GraceLimit,
            banned_at_ms: now,
            cost: Some(cost),
            unbanned_at_ms: None,
        });
        self.after_peer();
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

    /// Notes that the ban on `node` was lifted, so what it cost before
    /// does not count again.
    pub(super) fn record_unban(&mut self, node: NodeId) {
        let now = self.now_ms;
        if let Some(record) = self.ban_records.get_mut(&node) {
            record.unbanned_at_ms = Some(now.max(record.banned_at_ms));
            self.batch
                .bans
                .push(BanChange::Unbanned { node, at_ms: now });
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
        assert_eq!(
            grace_limit(LedgerId::StubUsd, u64::MAX, 1_000_000),
            u64::MAX
        );
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
