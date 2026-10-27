//! What the issuer of a token hub's asset does to the escrow
//! (spec/decisions/token-hubs.md), as validators attest it: its hold on the
//! escrow (a freeze of the escrow's line or of every line, a transfer fee)
//! and its clawbacks.
//!
//! - **A hold is a state, newest first.** Validators attest the state as of
//!   the ledger in which it last changed, and a quorum's state replaces the
//!   one in force only if its ledger is later, so a late vote for an older
//!   state, a faulty validator's among others, never takes the hub back to
//!   it. Each validator has one vote, as for new rules, which only moves
//!   forward. One attestation carries both a freeze and a fee, so a late
//!   thaw of the line never undoes a fee set after it.
//! - **Deadlines move at the thaw,** rather than running on a clock of the
//!   hub's own: every deadline the state holds stays a block timestamp,
//!   which members and the payout set read as they are.
//! - **A frozen hub's clock stands still.** From the block a quorum's hold
//!   took effect in, the ledger takes no member's operation but claims of
//!   commitments that expire within the freeze's window, credits no deposit
//!   and settles no withdrawal, though it records their attestations; every
//!   notice and a validator's absence run by the hub's clock
//!   ([`Ledger::clock`]), which stays at the freeze's stamp. The thaw moves
//!   every deadline still to come back by the freeze's length, in one pass
//!   over tables the rules bound, so each runs for what was left of it.
//! - **A clawback ends the hub.** Each is a transaction, attested as a
//!   deposit is, and counted once: what it took is kept apart from what the
//!   hub owes ([`Escrow::clawed_back`](super::Escrow::clawed_back)), and the
//!   hub shuts down, with the shutdown's notice for payees to take what
//!   their holds owe them. The payout set then pays the withdrawals already
//!   sequenced first, and every account pro rata
//!   (`escrow::payout`).

use super::Ledger;
use super::outcome::{Detail, Failure, Refusal};
use super::state::Phase;
use super::validator::vote;
use crate::consensus::Config;
use crate::digest::Digest;
use crate::operation::{Clawback, Issuer, MemberOperation};
use crate::rules::{Asset, MAX_HOLD_MS, Millis};
use crate::validators::{Signers, ValidatorIndex};

type Attested = Result<Detail, Result<Failure, Refusal>>;

/// The most clawbacks the hub remembers against counting one twice. When
/// the table is full, the clawbacks of its lowest ledger go, and the hub
/// takes no attestation of a clawback below it, as for deposits. The first
/// clawback ends the hub, so only those of its shutdown's notice follow.
pub const MAX_CLAWBACKS: usize = 16;

/// The most clawbacks one validator may have attested that no quorum
/// attested yet: a faulty validator's made-up clawbacks fill only its own
/// share, and the table holds at most this many per validator.
pub const CLAWBACK_SHARE: usize = 4;

impl Ledger {
    /// Whether the hub holds an issued token, whose issuer can freeze and
    /// claw back.
    fn token_hub(&self) -> bool {
        !matches!(self.rules.terms.asset, Asset::Xrp)
    }

    /// The last expiry a commitment claimed while the hub is frozen may
    /// have: the freeze's stamp, plus the margin M, the longest a connector
    /// holds a packet and the precision. That covers every packet a payee
    /// took up to M after the freeze's stamp, since a payee takes no
    /// commitment that expires further ahead of its clock than the longest
    /// hold and the precision (spec/decisions/token-hubs.md). `None` while
    /// the hub is not frozen.
    #[must_use]
    pub fn freeze_window_end(&self) -> Option<Millis> {
        let since = self.frozen?;
        Some(
            since
                .saturating_add(self.rules.terms.timers.margin)
                .saturating_add(MAX_HOLD_MS)
                .saturating_add(Config::DEFAULT.precision_ms),
        )
    }

    /// Whether a member's operation waits for the thaw: every operation
    /// while the hub is frozen, but a claim whose commitments all expire
    /// within the freeze's window. One that waits is refused
    /// ([`Refusal::Frozen`]): it pays nothing and uses no sequence, so the
    /// member can send it again after the thaw.
    #[must_use]
    pub fn waits(&self, operation: &MemberOperation) -> bool {
        let Some(end) = self.freeze_window_end() else {
            return false;
        };
        match operation {
            MemberOperation::Claim { fulfilled, .. } => fulfilled
                .iter()
                .any(|item| item.commitment.value.expiry > end),
            _ => true,
        }
    }

    /// A validator's vote for the issuer's hold as of a ledger. Only a
    /// token hub takes it, and only for a ledger after the state in force.
    pub(super) fn attest_issuer(&mut self, index: ValidatorIndex, issuer: Issuer) -> Attested {
        if !self.token_hub() {
            return Err(Err(Refusal::NotAllowed));
        }
        // A validator's vote only moves forward: one for an older state
        // than it voted for already came late, and would take its vote off
        // the state the others are agreeing on.
        let voted = self
            .issuer_votes
            .iter()
            .find(|(_, signers)| signers.contains(index))
            .map(|(v, _)| v.ledger);
        if issuer.ledger <= self.issuer.ledger || voted.is_some_and(|v| issuer.ledger < v) {
            return Err(Ok(Failure::Stale));
        }
        let quorum = self.validators.quorum();
        vote(&mut self.issuer_votes, index, issuer, quorum).map_err(Err)?;
        self.settle_issuer();
        Ok(Detail::None)
    }

    /// Puts in force the latest state of the issuer's hold a quorum agrees
    /// on, if any, and drops the votes for states no later than it: the
    /// hub freezes at this block if it now holds the hub, and thaws if it
    /// no longer does. Run after each attestation and at the end of every
    /// block, since a change of validators can give votes already cast a
    /// quorum.
    pub(super) fn settle_issuer(&mut self) {
        let quorum = self.validators.quorum();
        let agreed = self
            .issuer_votes
            .iter()
            .filter(|(_, signers)| signers.count() >= quorum)
            .map(|(issuer, _)| *issuer)
            .max_by_key(|issuer| issuer.ledger);
        let Some(state) = agreed else {
            return;
        };
        self.issuer = state;
        self.issuer_votes
            .retain(|(issuer, _)| issuer.ledger > state.ledger);
        match (state.holds(), self.frozen) {
            (true, None) => self.frozen = Some(self.timestamp),
            (false, Some(since)) => self.thaw(since),
            _ => {}
        }
    }

    /// Ends a freeze that began at the block stamped `since`: every
    /// deadline the hub's clock still runs toward moves back by the
    /// freeze's length, so nothing fell due during it and each runs for
    /// what was left of it. Deadlines a freeze found passed applied at its
    /// block, before it took effect.
    pub(super) fn thaw(&mut self, since: Millis) {
        let length = self.timestamp.saturating_sub(since);
        self.frozen = None;
        for hold in self.holds.values_mut() {
            if let Some(reduction) = hold.reduction.as_mut() {
                reduction.at = reduction.at.saturating_add(length);
            }
        }
        if let Some(pending) = self.pending_rules.as_mut()
            && pending.from_height.is_none()
        {
            pending.at = pending.at.saturating_add(length);
        }
        if let Phase::ShuttingDown { ends } = &mut self.phase {
            *ends = ends.saturating_add(length);
        }
        for state in &mut self.validator_states {
            state.last_recorded = state.last_recorded.saturating_add(length);
        }
    }

    /// A validator's attestation of a clawback from the escrow. Only a
    /// token hub takes it. Like a deposit's, a clawback becomes pending with
    /// its first attestation, and below the floor it does not; one
    /// remembered is applied already.
    pub(super) fn attest_clawback(
        &mut self,
        index: ValidatorIndex,
        clawback: &Clawback,
    ) -> Attested {
        if !self.token_hub() {
            return Err(Err(Refusal::NotAllowed));
        }
        let hash = clawback.transaction;
        if self.recent_clawbacks.contains_key(&hash) {
            return Ok(Detail::None);
        }
        let same = |c: &Clawback| c.transaction == hash;
        if self
            .clawback_votes
            .iter()
            .any(|(c, signers)| same(c) && signers.contains(index))
        {
            return Err(Err(Refusal::NotAllowed));
        }
        let pending = self.clawback_votes.iter().any(|(c, _)| same(c));
        if !pending && clawback.ledger < self.escrow.clawback_floor {
            return Err(Ok(Failure::Stale));
        }
        let mine = self
            .clawback_votes
            .iter()
            .filter(|(_, signers)| signers.contains(index))
            .count();
        if mine >= CLAWBACK_SHARE {
            return Err(Ok(Failure::Full));
        }
        match self.clawback_votes.iter_mut().find(|(c, _)| c == clawback) {
            Some((_, signers)) => *signers = signers.with(index),
            None => self
                .clawback_votes
                .push((*clawback, Signers::NONE.with(index))),
        }
        self.settle_clawbacks();
        Ok(Detail::None)
    }

    /// Applies every clawback a quorum agrees on: what it took is kept
    /// apart, it is remembered, and the hub shuts down if it was running.
    /// Run after each attestation and at the end of every block.
    pub(super) fn settle_clawbacks(&mut self) {
        let quorum = self.validators.quorum();
        let agreed: alloc::vec::Vec<Clawback> = self
            .clawback_votes
            .iter()
            .filter(|(_, signers)| signers.count() >= quorum)
            .map(|(clawback, _)| *clawback)
            .collect();
        for clawback in agreed {
            if self.recent_clawbacks.contains_key(&clawback.transaction) {
                continue;
            }
            self.clawback_votes
                .retain(|(c, _)| c.transaction != clawback.transaction);
            self.escrow.clawed_back = self.escrow.clawed_back.saturating_add(clawback.amount);
            self.remember_clawback(clawback.transaction, clawback.ledger);
            if self.phase == Phase::Running {
                self.begin_shutdown();
            }
        }
    }

    /// Remembers an applied clawback against counting it twice, raising
    /// the floor past the lowest ledger when the table is full.
    fn remember_clawback(&mut self, hash: Digest, ledger: u32) {
        while self.recent_clawbacks.len() >= MAX_CLAWBACKS {
            let Some(lowest) = self.recent_clawbacks.values().min().copied() else {
                break;
            };
            self.recent_clawbacks.retain(|_, l| *l != lowest);
            self.escrow.clawback_floor = self.escrow.clawback_floor.max(lowest.saturating_add(1));
        }
        self.recent_clawbacks.insert(hash, ledger);
    }

    /// Forgets the issuer's acts on the chain a reset of the test network
    /// left behind: the hub thaws, and what was clawed back is written off
    /// with everything else.
    pub(super) fn write_off_issuer(&mut self) {
        if let Some(since) = self.frozen {
            self.thaw(since);
        }
        self.issuer = Issuer::default();
        self.issuer_votes.clear();
        self.clawback_votes.clear();
        self.recent_clawbacks.clear();
        self.escrow.clawed_back = 0;
        self.escrow.clawback_floor = 0;
    }

    /// Asserts what the issuer's acts leave: only a token hub has any; the
    /// hub is frozen exactly while the state in force holds it, never once
    /// it ended, and no deadline a frozen hub keeps lies before the freeze;
    /// votes are for newer states, and the tables are within their bounds.
    pub(super) fn check_issuer(&self) {
        if !self.token_hub() {
            assert!(
                self.issuer == Issuer::default()
                    && self.issuer_votes.is_empty()
                    && self.clawback_votes.is_empty()
                    && self.recent_clawbacks.is_empty()
                    && self.escrow.clawed_back == 0,
                "an issuer's act in an XRP hub"
            );
        }
        assert_eq!(
            self.frozen.is_some(),
            self.issuer.holds(),
            "frozen otherwise than the issuer holds the hub"
        );
        if let Some(since) = self.frozen {
            assert!(since <= self.timestamp, "frozen from a later block");
            assert!(self.phase != Phase::Ended, "a frozen hub ended");
            assert!(
                self.validator_states
                    .iter()
                    .all(|v| v.last_recorded <= since),
                "a vote recorded after the hub's clock"
            );
        }
        if self.escrow.clawed_back > 0 {
            assert!(self.phase != Phase::Running, "clawed back and running");
        }
        let n = self.validators.len();
        assert!(self.issuer_votes.len() <= n, "issuer votes beyond the set");
        assert!(
            self.issuer_votes
                .iter()
                .all(|(issuer, signers)| issuer.ledger > self.issuer.ledger
                    && signers.count() > 0
                    && signers.fits(n)),
            "a vote for a state no newer than the one in force"
        );
        for (index, _) in self.validators.iter() {
            let mine = self
                .clawback_votes
                .iter()
                .filter(|(_, signers)| signers.contains(index))
                .count();
            assert!(mine <= CLAWBACK_SHARE, "clawback votes beyond a share");
        }
        assert!(
            self.clawback_votes
                .iter()
                .all(|(c, signers)| signers.count() > 0
                    && signers.fits(n)
                    && !self.recent_clawbacks.contains_key(&c.transaction)),
            "a clawback pending and applied"
        );
        assert!(
            self.recent_clawbacks.len() <= MAX_CLAWBACKS
                && self
                    .recent_clawbacks
                    .values()
                    .all(|ledger| *ledger >= self.escrow.clawback_floor),
            "remembered clawbacks beyond their bound or below the floor"
        );
    }
}
