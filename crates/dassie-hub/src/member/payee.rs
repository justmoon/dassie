//! The payee's side of a hold a peer set aside for us
//! (spec/designs/settlement-hubs.md#4-peering-over-a-hub): the commitment
//! each Prepare from it carries, what we fulfilled, the claims it sends,
//! and the claims we make on the hub when its own do not come.
//!
//! - **A commitment holds** when the payer's account key signed it, for its
//!   hold toward us, for exactly what the Prepare says, and the hold covers
//!   it beyond what we have not taken and what is still in flight. Past
//!   [`MAX_MISSING`] packets its claims left uncovered, we take no more of
//!   its Prepares.
//! - **A claim holds** when it is signed, for that hold, only grows, covers
//!   no packet still in flight, and its T is what we fulfilled up to its W
//!   by our own record, in internal units rounded down to drops. A packet
//!   we fulfilled too late to claim on the hub may be left out of it: its
//!   commitment expired, so only the payer can still pay it, and a claim
//!   without it is still worth taking.
//! - **What we fulfilled and no claim covers** we claim on the hub with its
//!   commitment and fulfillment once the payer's claim is late: a reply
//!   leg's allowance after every packet below it was resolved, so that the
//!   payer could have covered it, and in any case a margin before its
//!   commitment expires, so the claim lands in a block stamped before.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_identity::PublicKey;
use dassie_settlement::{Promised, SchemeError};

use super::payer::Snapshot;
use super::{UNITS_PER_DROP, drops_covering};
use crate::promise::{Claim, Commitment, Fulfilled, HoldNumber, Signed};
use crate::rules::{Amount, HubId, Millis};

/// The most packets a payer's claims may leave uncovered before we take
/// no more of its Prepares (spec/designs/settlement-hubs.md#4-peering-over-a-hub):
/// the hub keeps at least this many claimed sequences per hold.
pub const MAX_MISSING: usize = 1_024;
/// Most of its packets in flight through us at once.
pub(super) const MAX_IN_FLIGHT: usize = 1_024;

/// Where our claim on the hub for a packet stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Claiming {
    /// Not claimed.
    No,
    /// In a claim on its way.
    Submitted,
    /// The hub paid it.
    Paid,
    /// Its commitment expired unpaid: only the payer can pay it now.
    Lost,
}

/// A packet we fulfilled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Done {
    pub(super) fulfillment: [u8; 32],
    /// When every packet below it was resolved, so the payer could cover
    /// it: from then on its claim is late after a reply leg.
    pub(super) ready_ms: Option<u64>,
    pub(super) claiming: Claiming,
    /// Claims of ours on the hub that failed with it in them.
    pub(super) tries: u8,
}

/// Most claims on the hub that may fail with a packet in them: each costs
/// a fee. After the first, it is claimed alone, so that one packet the hub
/// will not pay keeps no other from being paid.
pub(super) const MAX_TRIES: u8 = 2;

/// A packet of the payer's we took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Taken {
    pub(super) request_id: Option<u32>,
    pub(super) commitment: Signed<Commitment>,
    /// The Prepare's amount, in internal units.
    pub(super) units: u64,
    /// Set once we fulfilled it.
    pub(super) done: Option<Done>,
}

/// A hold a peer set aside for us.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Payee {
    pub(super) hold: Option<HoldNumber>,
    /// An executed operation of the payer's on the hold verified under the
    /// key it gave us: the hub would take its commitments.
    pub(super) key_confirmed: bool,
    /// The newest claim of the payer's we took.
    pub(super) accepted: Option<Signed<Claim>>,
    /// What we count as owed up to its W, in internal units.
    pub(super) base_units: u128,
    /// The packets above its W.
    pub(super) taken: BTreeMap<u64, Taken>,
    pub(super) requests: BTreeMap<u32, u64>,
    pub(super) snapshot: Snapshot,
}

impl Payee {
    /// W of the claim we took.
    fn through(&self) -> u64 {
        self.accepted.map_or(0, |claim| claim.value.through)
    }

    /// Packets its claims left uncovered.
    pub(super) fn missing(&self) -> usize {
        self.taken.values().filter(|t| t.done.is_some()).count()
    }

    fn in_flight(&self) -> usize {
        self.taken.values().filter(|t| t.done.is_none()).count()
    }

    /// What we have not taken from the hold, in drops: everything owed,
    /// less what we took.
    fn untaken(&self) -> Amount {
        let owed = self
            .taken
            .values()
            .filter(|t| t.done.is_some())
            .map(|t| u128::from(t.units))
            .fold(self.base_units, u128::saturating_add);
        let owed = owed.div_ceil(u128::from(UNITS_PER_DROP));
        Amount::try_from(owed)
            .unwrap_or(Amount::MAX)
            .saturating_sub(self.snapshot.taken)
    }

    /// What the hold has room for a packet expiring at `expiry`: what is
    /// left in it once a reduction due by then took effect, less what we
    /// have not taken and what is still in flight.
    fn room(&self, expiry: Millis) -> Amount {
        let reduction = self
            .snapshot
            .reduction
            .filter(|(_, at)| *at <= expiry)
            .map_or(0, |(amount, _)| amount);
        let in_flight = self
            .taken
            .values()
            .filter(|t| t.done.is_none())
            .map(|t| t.commitment.value.amount)
            .fold(0, Amount::saturating_add);
        self.snapshot
            .amount
            .saturating_sub(reduction)
            .saturating_sub(self.untaken())
            .saturating_sub(in_flight)
    }

    /// Checks the payer's commitment for its Prepare `request_id`, and
    /// takes the packet if it holds.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidData`] for a commitment that does not hold;
    /// [`SchemeError::Unavailable`] while the hold cannot cover it, or the
    /// payer's claims left too many packets uncovered.
    pub(super) fn check(
        &mut self,
        request_id: u32,
        promised: Promised,
        signed: Signed<Commitment>,
        (hub, payer, margin): (&HubId, &PublicKey, Millis),
    ) -> Result<(), SchemeError> {
        let commitment = signed.value;
        let expiry = promised.expires_at_ms.checked_sub(margin);
        let holds = Some(commitment.hold) == self.hold
            && self.key_confirmed
            && commitment.amount == drops_covering(promised.amount)
            && commitment.condition == promised.condition
            && Some(commitment.expiry) == expiry
            && commitment.sequence > self.through()
            && commitment.sequence > self.snapshot.through
            && !self.taken.contains_key(&commitment.sequence)
            && !self.requests.contains_key(&request_id)
            && signed.verifies(hub, payer);
        if !holds {
            return Err(SchemeError::InvalidData);
        }
        let full = self.missing() >= MAX_MISSING || self.in_flight() >= MAX_IN_FLIGHT;
        if full || self.room(commitment.expiry) < commitment.amount {
            return Err(SchemeError::Unavailable);
        }
        self.taken.insert(
            commitment.sequence,
            Taken {
                request_id: Some(request_id),
                commitment: signed,
                units: promised.amount,
                done: None,
            },
        );
        self.requests.insert(request_id, commitment.sequence);
        Ok(())
    }

    /// We fulfilled its Prepare `request_id` with `fulfillment`.
    pub(super) fn fulfilled(&mut self, now_ms: u64, request_id: u32, fulfillment: [u8; 32]) {
        let Some(sequence) = self.requests.remove(&request_id) else {
            return;
        };
        if let Some(taken) = self.taken.get_mut(&sequence) {
            taken.request_id = None;
            if taken.commitment.value.fulfilled_by(&fulfillment) {
                taken.done = Some(Done {
                    fulfillment,
                    ready_ms: None,
                    claiming: Claiming::No,
                    tries: 0,
                });
            } else {
                self.taken.remove(&sequence);
            }
        }
        self.mark_ready(now_ms);
    }

    /// Its Prepare `request_id` was rejected or expired.
    pub(super) fn released(&mut self, now_ms: u64, request_id: u32) {
        let Some(sequence) = self.requests.remove(&request_id) else {
            return;
        };
        if self.taken.get(&sequence).is_some_and(|t| t.done.is_none()) {
            self.taken.remove(&sequence);
        }
        self.mark_ready(now_ms);
    }

    /// Marks the packets we fulfilled whose claim the payer can make now:
    /// every packet below them is resolved.
    fn mark_ready(&mut self, now_ms: u64) {
        for taken in self.taken.values_mut() {
            let Some(done) = &mut taken.done else {
                return;
            };
            done.ready_ms.get_or_insert(now_ms);
        }
    }

    /// Takes the payer's claim if it holds. Returns whether it did.
    pub(super) fn claim(&mut self, signed: Signed<Claim>, hub: &HubId, payer: &PublicKey) -> bool {
        let claim = signed.value;
        let (total, through) = self
            .accepted
            .map_or((0, 0), |c| (c.value.total, c.value.through));
        if Some(claim.hold) != self.hold
            || claim.total < total
            || claim.through < through
            || !signed.verifies(hub, payer)
        {
            return false;
        }
        let covered = self.taken.range(..=claim.through);
        let mut all = self.base_units;
        let mut lost = 0u128;
        for (_, taken) in covered {
            let Some(done) = taken.done else {
                // A packet still in flight: its claim may yet be ours to
                // make on the hub, which a redeem through it would forbid.
                return false;
            };
            all = all.saturating_add(u128::from(taken.units));
            if done.claiming == Claiming::Lost {
                lost = lost.saturating_add(u128::from(taken.units));
            }
        }
        let drops = |units: u128| units / u128::from(UNITS_PER_DROP);
        let base = if u128::from(claim.total) == drops(all) {
            all
        } else if u128::from(claim.total) == drops(all.saturating_sub(lost)) {
            all.saturating_sub(lost)
        } else {
            return false;
        };
        self.base_units = base;
        self.accepted = Some(signed);
        let rest = self.taken.split_off(&claim.through.saturating_add(1));
        self.taken = rest;
        self.requests
            .retain(|_, sequence| *sequence > claim.through);
        true
    }

    /// The packets to claim on the hub now, in sequence order: what we
    /// fulfilled, no claim covers and no claim of ours is on its way for,
    /// whose payer's claim is late at `now_ms`. At most `limit`; one that
    /// was in a claim that failed goes alone.
    pub(super) fn due(
        &self,
        now_ms: u64,
        (allowance, margin): (Millis, Millis),
        limit: usize,
    ) -> Vec<Fulfilled> {
        let due: Vec<(u8, Fulfilled)> = self
            .taken
            .values()
            .filter_map(|taken| {
                let done = taken.done?;
                if done.claiming != Claiming::No || taken.commitment.value.expiry <= now_ms {
                    return None;
                }
                let fulfilled = Fulfilled {
                    commitment: taken.commitment,
                    fulfillment: done.fulfillment,
                };
                (deadline(taken, &done, allowance, margin) <= now_ms)
                    .then_some((done.tries, fulfilled))
            })
            .collect();
        if let Some((_, retry)) = due.iter().find(|(tries, _)| *tries > 0) {
            return alloc::vec![*retry];
        }
        due.into_iter().map(|(_, f)| f).take(limit).collect()
    }

    /// A claim of ours with the packets `sequences` failed: each may be
    /// claimed again, alone, until it failed [`MAX_TRIES`] times.
    pub(super) fn claim_failed(&mut self, sequences: &[u64]) {
        for sequence in sequences {
            if let Some(done) = self.taken.get_mut(sequence).and_then(|t| t.done.as_mut()) {
                done.tries = done.tries.saturating_add(1);
                done.claiming = if done.tries >= MAX_TRIES {
                    Claiming::Lost
                } else {
                    Claiming::No
                };
            }
        }
    }

    /// When the next packet falls due to be claimed on the hub.
    pub(super) fn next_due(&self, (allowance, margin): (Millis, Millis)) -> Option<u64> {
        self.taken
            .values()
            .filter_map(|taken| {
                let done = taken.done?;
                (done.claiming == Claiming::No).then(|| deadline(taken, &done, allowance, margin))
            })
            .min()
    }

    /// Sets the claim state of the packets `sequences`, those we still
    /// have.
    pub(super) fn set_claiming(&mut self, sequences: &[u64], claiming: Claiming) {
        for sequence in sequences {
            if let Some(done) = self.taken.get_mut(sequence).and_then(|t| t.done.as_mut()) {
                done.claiming = claiming;
            }
        }
    }

    /// A final block stamped `timestamp`: what we fulfilled and the hub did
    /// not pay before its commitment expired is lost to a claim.
    pub(super) fn final_block(&mut self, timestamp: Millis) {
        for taken in self.taken.values_mut() {
            if let Some(done) = &mut taken.done
                && matches!(done.claiming, Claiming::No | Claiming::Submitted)
                && taken.commitment.value.expiry <= timestamp
            {
                done.claiming = Claiming::Lost;
            }
        }
    }

    /// Asserts how its parts relate.
    pub(super) fn check_invariants(&self) {
        assert!(self.missing() <= MAX_MISSING);
        assert!(self.in_flight() <= MAX_IN_FLIGHT);
        let through = self.through();
        assert!(
            self.taken.keys().all(|sequence| *sequence > through),
            "what a claim covers is forgotten"
        );
        for (request_id, sequence) in &self.requests {
            assert!(
                self.taken
                    .get(sequence)
                    .is_some_and(|t| t.request_id == Some(*request_id) && t.done.is_none()),
                "a request names a packet in flight"
            );
        }
        if let (Some(hold), Some(claim)) = (self.hold, self.accepted) {
            assert_eq!(claim.value.hold, hold, "a claim on the hold");
        }
    }
}

/// When the payer's claim for a packet we fulfilled is late: a reply leg
/// after the payer could make it, and a margin before its commitment
/// expires in any case.
fn deadline(taken: &Taken, done: &Done, allowance: Millis, margin: Millis) -> u64 {
    let last = taken.commitment.value.expiry.saturating_sub(margin);
    done.ready_ms
        .map_or(last, |ready| ready.saturating_add(allowance).min(last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;
    use dassie_identity::NodeKey;
    use sha2::{Digest as _, Sha256};

    const MARGIN: Millis = 500;
    const ALLOWANCE: Millis = 300;

    fn hub() -> HubId {
        HubId(Digest([1; 32]))
    }

    fn key() -> NodeKey {
        NodeKey::from_bytes(&[2; 32])
    }

    fn payee(amount: Amount) -> Payee {
        Payee {
            hold: Some(HoldNumber(3)),
            key_confirmed: true,
            snapshot: Snapshot {
                amount,
                ..Snapshot::default()
            },
            ..Payee::default()
        }
    }

    fn promised(units: u64, preimage: u8) -> Promised {
        Promised {
            amount: units,
            condition: Sha256::digest([preimage; 32]).into(),
            expires_at_ms: 10_000,
        }
    }

    fn commitment(sequence: u64, promised: Promised) -> Signed<Commitment> {
        let value = Commitment {
            hold: HoldNumber(3),
            sequence,
            amount: drops_covering(promised.amount),
            condition: promised.condition,
            expiry: promised.expires_at_ms - MARGIN,
        };
        Signed::sign(value, &hub(), &key())
    }

    fn take(
        payee: &mut Payee,
        request_id: u32,
        sequence: u64,
        units: u64,
    ) -> Result<(), SchemeError> {
        let promised = promised(units, u8::try_from(sequence % 256).unwrap());
        let signed = commitment(sequence, promised);
        payee.check(
            request_id,
            promised,
            signed,
            (&hub(), &key().public_key(), MARGIN),
        )
    }

    fn claim(total: Amount, through: u64) -> Signed<Claim> {
        Signed::sign(
            Claim {
                hold: HoldNumber(3),
                total,
                through,
            },
            &hub(),
            &key(),
        )
    }

    // A commitment holds only for exactly what its Prepare says, signed by
    // the payer, on its hold.
    #[test]
    fn a_commitment_must_match_its_prepare() {
        let mut payee = payee(1_000);
        let promised = promised(1_000, 1);
        let context = (&hub(), &key().public_key(), MARGIN);
        let mut wrong = commitment(1, promised);
        wrong.value.expiry += 1;
        assert_eq!(
            payee.check(1, promised, wrong, context),
            Err(SchemeError::InvalidData)
        );
        let mut wrong = commitment(1, promised);
        wrong.value.amount -= 1;
        assert_eq!(
            payee.check(1, promised, wrong, context),
            Err(SchemeError::InvalidData)
        );
        let other = Signed::sign(
            commitment(1, promised).value,
            &hub(),
            &NodeKey::from_bytes(&[9; 32]),
        );
        assert_eq!(
            payee.check(1, promised, other, context),
            Err(SchemeError::InvalidData)
        );
        let mut other_hold = commitment(1, promised);
        other_hold.value.hold = HoldNumber(4);
        assert_eq!(
            payee.check(1, promised, other_hold, context),
            Err(SchemeError::InvalidData)
        );
        payee
            .check(1, promised, commitment(1, promised), context)
            .unwrap();
        assert_eq!(
            payee.check(2, promised, commitment(1, promised), context),
            Err(SchemeError::InvalidData),
            "a sequence once"
        );
        payee.check_invariants();
    }

    #[test]
    fn an_unconfirmed_key_takes_nothing() {
        let mut payee = payee(1_000);
        payee.key_confirmed = false;
        assert_eq!(take(&mut payee, 1, 1, 1_000), Err(SchemeError::InvalidData));
    }

    // A packet fits when the hold covers it beyond what we have not taken
    // and what is in flight, and a reduction due before it expires.
    #[test]
    fn a_packet_fits_in_what_the_hold_has_left() {
        let mut payee = payee(10);
        take(&mut payee, 1, 1, 4_000).unwrap();
        payee.fulfilled(0, 1, [1; 32]);
        take(&mut payee, 2, 2, 4_000).unwrap();
        assert_eq!(take(&mut payee, 3, 3, 2_001), Err(SchemeError::Unavailable));
        take(&mut payee, 3, 3, 2_000).unwrap();
        let mut reduced = self::payee(10);
        reduced.snapshot.reduction = Some((5, 9_000));
        assert_eq!(
            take(&mut reduced, 1, 1, 6_000),
            Err(SchemeError::Unavailable)
        );
        payee.check_invariants();
    }

    // spec/designs/settlement-hubs.md#4-peering-over-a-hub: past 1024
    // packets left uncovered by the payer's claims, its Prepares are
    // refused.
    #[test]
    fn too_many_missing_claims_stop_the_payer() {
        let mut payee = payee(u64::MAX / 2);
        for sequence in 1..=1_024u32 {
            take(&mut payee, sequence, u64::from(sequence), 1_000).unwrap();
            payee.fulfilled(
                0,
                sequence,
                [u8::try_from(u64::from(sequence) % 256).unwrap(); 32],
            );
        }
        assert_eq!(payee.missing(), 1_024);
        assert_eq!(
            take(&mut payee, 2_000, 2_000, 1_000),
            Err(SchemeError::Unavailable)
        );
    }

    // A claim is taken when its T is what we fulfilled up to its W.
    #[test]
    fn a_claim_must_match_what_we_fulfilled() {
        let mut payee = payee(1_000);
        take(&mut payee, 1, 1, 1_500).unwrap();
        take(&mut payee, 2, 2, 1_500).unwrap();
        payee.fulfilled(0, 1, [1; 32]);
        assert!(
            !payee.claim(claim(1, 2), &hub(), &key().public_key()),
            "2 is in flight"
        );
        assert!(
            !payee.claim(claim(2, 1), &hub(), &key().public_key()),
            "too much"
        );
        assert!(payee.claim(claim(1, 1), &hub(), &key().public_key()));
        payee.fulfilled(0, 2, [2; 32]);
        assert!(
            !payee.claim(claim(2, 2), &hub(), &key().public_key()),
            "3 units are 3 drops"
        );
        assert!(payee.claim(claim(3, 2), &hub(), &key().public_key()));
        assert!(payee.taken.is_empty());
        assert!(
            !payee.claim(claim(3, 1), &hub(), &key().public_key()),
            "W never shrinks"
        );
        payee.check_invariants();
    }

    #[test]
    fn a_claim_may_leave_out_what_we_can_no_longer_claim() {
        let mut payee = payee(1_000);
        take(&mut payee, 1, 1, 1_000).unwrap();
        payee.fulfilled(0, 1, [1; 32]);
        payee.final_block(10_000);
        assert!(payee.claim(claim(0, 1), &hub(), &key().public_key()));
        assert_eq!(payee.base_units, 0);
    }

    // A claim the hub refused costs a fee: what was in it is claimed again
    // alone, so one bad packet holds no other back, and only so often.
    #[test]
    fn a_failed_claim_is_tried_again_alone_and_only_so_often() {
        let mut payee = payee(1_000);
        for sequence in 1..=3 {
            take(&mut payee, sequence, u64::from(sequence), 1_000).unwrap();
            payee.fulfilled(0, sequence, [u8::try_from(sequence).unwrap(); 32]);
        }
        let context = (ALLOWANCE, MARGIN);
        assert_eq!(payee.due(1_000, context, 8).len(), 3);
        payee.set_claiming(&[1, 2, 3], Claiming::Submitted);
        payee.claim_failed(&[1, 2, 3]);
        let retry = payee.due(1_000, context, 8);
        assert_eq!(retry.len(), 1, "alone");
        assert_eq!(retry[0].commitment.value.sequence, 1);
        payee.claim_failed(&[1]);
        assert_eq!(payee.taken[&1].done.unwrap().claiming, Claiming::Lost);
        assert_eq!(payee.due(1_000, context, 8)[0].commitment.value.sequence, 2);
        payee.check_invariants();
    }

    // A claim on the hub is due a reply leg after the payer could have
    // claimed, and a margin before the commitment expires at the latest.
    #[test]
    fn missing_claims_fall_due() {
        let mut payee = payee(1_000);
        take(&mut payee, 1, 1, 1_000).unwrap();
        take(&mut payee, 2, 2, 1_000).unwrap();
        payee.fulfilled(1_000, 2, [2; 32]);
        let context = (ALLOWANCE, MARGIN);
        // 1 is in flight: the payer cannot cover 2 yet.
        assert_eq!(payee.next_due(context), Some(10_000 - 2 * MARGIN));
        payee.released(2_000, 1);
        assert_eq!(payee.next_due(context), Some(2_000 + ALLOWANCE));
        assert!(payee.due(2_299, context, 8).is_empty());
        let due = payee.due(2_300, context, 8);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].commitment.value.sequence, 2);
        payee.set_claiming(&[2], Claiming::Submitted);
        assert!(payee.due(3_000, context, 8).is_empty());
        payee.check_invariants();
    }
}
