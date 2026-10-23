//! The payer's side of a hold toward a peer
//! (spec/designs/settlement-hubs.md#4-peering-over-a-hub): a commitment with
//! each Prepare, a claim with each reply to a Fulfill, and what the hub
//! decides of packets whose Reject upstream waits for it.
//!
//! Each packet takes the hold's next sequence. A packet is resolved once
//! its Fulfill was answered, it was rejected, or the hub decided it; W is
//! the highest sequence up to which every packet is resolved, and T what we
//! owe for the packets up to W: those whose Fulfill we took, and those the
//! hub paid the peer for with its claim, since the hub overrules our books
//! (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A packet above W
//! is never in T: the hub remembers what `claim` paid above the last
//! redeemed W, and only subtracts it from a redeem up to W, so a T that
//! counted it would pay it twice.
//!
//! Amounts: a packet is promised in drops, rounded up, so a commitment
//! never covers less than its packet; T is what the packets add up to in
//! internal units, rounded down to drops, so what is less than a drop
//! stays owed until there is more, as in every settlement.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_settlement::{LossCause, Promised, SchemeError};

use super::{UNITS_PER_DROP, drops_covering};
use crate::digest::Digest;
use crate::promise::{Claim, Commitment, Fulfilled, HoldNumber};
use crate::rules::{Amount, Millis};

/// Most packets a payer keeps per peer: in flight, waiting for the hub, or
/// resolved and still claimable on the hub.
pub(super) const MAX_PACKETS: usize = 2_048;

/// Where a packet of ours stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    /// On its way to the peer.
    InFlight,
    /// It expired, and its Reject upstream waits for the hub.
    Waiting,
    /// The peer fulfilled it; `taken` whether the books took the Fulfill.
    Fulfilled { taken: bool },
    /// The peer rejected it.
    Rejected,
    /// It waited for the hub until the node rejected it upstream.
    GaveUp,
    /// The hub decided it while it waited.
    Decided,
}

impl State {
    const fn resolved(self) -> bool {
        !matches!(self, Self::InFlight | Self::Waiting)
    }
}

/// What the hub paid the peer for a packet with its claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PaidOnHub {
    /// The fulfillment the claim revealed.
    pub(super) fulfillment: [u8; 32],
    /// What it paid, in drops.
    pub(super) drops: Amount,
    /// The operation that paid it.
    pub(super) operation: Digest,
}

/// A packet of ours to the peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Packet {
    /// The request it went with, while the node is not done with it.
    pub(super) request_id: Option<u32>,
    /// Its amount, in internal units.
    pub(super) units: u64,
    /// What we signed for it.
    pub(super) commitment: Commitment,
    pub(super) state: State,
    pub(super) paid: Option<PaidOnHub>,
}

impl Packet {
    /// Whether we owe it: we took its Fulfill, or the hub paid it.
    const fn owed(&self) -> bool {
        self.paid.is_some() || matches!(self.state, State::Fulfilled { taken: true })
    }

    /// What it adds to what we owe, in internal units: the packet, or,
    /// where only the hub says, what it paid.
    const fn owed_units(&self) -> u128 {
        if !self.owed() {
            return 0;
        }
        self.units as u128
    }
}

/// What the hub's ledger last said of the hold, as the account mirrors it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    /// What is left in it.
    pub(super) amount: Amount,
    /// What the payee took from it.
    pub(super) taken: Amount,
    /// W of the last redeemed claim.
    pub(super) through: u64,
    /// A reduction waiting: how much, and from when.
    pub(super) reduction: Option<(Amount, Millis)>,
}

/// A loss a packet caused, booked at the next chance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Loss {
    pub(super) units: u64,
    pub(super) cause: LossCause,
    pub(super) operation: Digest,
}

/// Our hold toward a peer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Payer {
    pub(super) hold: Option<HoldNumber>,
    /// The sequence of the next commitment; the hub takes none at 0.
    pub(super) next_sequence: u64,
    /// W of our claims.
    pub(super) through: u64,
    /// What we owe for the packets up to W, in internal units.
    pub(super) through_units: u128,
    pub(super) packets: BTreeMap<u64, Packet>,
    pub(super) requests: BTreeMap<u32, u64>,
    pub(super) snapshot: Snapshot,
}

impl Payer {
    /// A payer with no hold yet.
    pub(super) fn new() -> Self {
        Self {
            next_sequence: 1,
            ..Self::default()
        }
    }

    /// T of our claims, in drops.
    pub(super) fn total(&self) -> Amount {
        Amount::try_from(self.through_units / u128::from(UNITS_PER_DROP)).unwrap_or(Amount::MAX)
    }

    /// The claim we stand by now.
    pub(super) fn claim_value(&self) -> Option<Claim> {
        Some(Claim {
            hold: self.hold?,
            total: self.total(),
            through: self.through,
        })
    }

    /// What the hold has room for, in internal units: what was put in it
    /// less what we owe on it and what is still on its way. A reduction
    /// waiting shrinks it already.
    pub(super) fn room(&self) -> u128 {
        let reduction = self.snapshot.reduction.map_or(0, |(amount, _)| amount);
        let funded = u128::from(
            self.snapshot
                .amount
                .saturating_sub(reduction)
                .saturating_add(self.snapshot.taken),
        ) * u128::from(UNITS_PER_DROP);
        let owed = self
            .packets
            .range(self.through.saturating_add(1)..)
            .map(|(_, packet)| packet.owed_units())
            .fold(self.through_units, u128::saturating_add);
        let open = self
            .packets
            .values()
            .filter(|packet| !packet.state.resolved())
            .map(|packet| u128::from(packet.commitment.amount) * u128::from(UNITS_PER_DROP))
            .fold(0, u128::saturating_add);
        funded.saturating_sub(owed).saturating_sub(open)
    }

    /// The commitment for our Prepare `request_id`, its sequence taken.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] without a hold, or room in it, or room
    /// to keep the packet; [`SchemeError::InvalidData`] for a request it
    /// has, or an expiry before the margin.
    pub(super) fn commit(
        &mut self,
        request_id: u32,
        promised: Promised,
        margin: Millis,
    ) -> Result<Commitment, SchemeError> {
        let hold = self.hold.ok_or(SchemeError::Unavailable)?;
        if self.requests.contains_key(&request_id) {
            return Err(SchemeError::InvalidData);
        }
        if self.packets.len() >= MAX_PACKETS {
            return Err(SchemeError::Unavailable);
        }
        let expiry = promised
            .expires_at_ms
            .checked_sub(margin)
            .ok_or(SchemeError::InvalidData)?;
        let amount = drops_covering(promised.amount);
        if self.room() < u128::from(amount) * u128::from(UNITS_PER_DROP) {
            return Err(SchemeError::Unavailable);
        }
        let sequence = self.next_sequence;
        self.next_sequence = sequence.checked_add(1).ok_or(SchemeError::Unavailable)?;
        let commitment = Commitment {
            hold,
            sequence,
            amount,
            condition: promised.condition,
            expiry,
        };
        self.packets.insert(
            sequence,
            Packet {
                request_id: Some(request_id),
                units: promised.amount,
                commitment,
                state: State::InFlight,
                paid: None,
            },
        );
        self.requests.insert(request_id, sequence);
        Ok(commitment)
    }

    /// The peer fulfilled our Prepare `request_id`, and the books took the
    /// Fulfill or not. Returns the claim to send, and a loss if the hub
    /// paid a Fulfill the books did not take.
    pub(super) fn fulfilled(
        &mut self,
        request_id: u32,
        taken: bool,
    ) -> (Option<Claim>, Option<Loss>) {
        let mut loss = None;
        if let Some(sequence) = self.requests.remove(&request_id)
            && let Some(packet) = self.packets.get_mut(&sequence)
        {
            packet.request_id = None;
            if !packet.state.resolved() {
                packet.state = State::Fulfilled { taken };
                if !taken {
                    loss = packet.paid.map(|paid| late(&paid, LossCause::PaidLate));
                }
            }
        }
        self.advance();
        (self.claim_value(), loss)
    }

    /// The node is done with our Prepare `request_id` without a Fulfill
    /// taken: the peer rejected it, or the node gave up waiting for the
    /// hub. Returns a loss if the hub paid it.
    pub(super) fn answered(&mut self, request_id: u32) -> Option<Loss> {
        let sequence = self.requests.remove(&request_id)?;
        let packet = self.packets.get_mut(&sequence)?;
        packet.request_id = None;
        let cause = match packet.state {
            State::InFlight => {
                packet.state = State::Rejected;
                LossCause::Cheated
            }
            State::Waiting => {
                packet.state = State::GaveUp;
                LossCause::PaidLate
            }
            _ => return None,
        };
        let loss = packet.paid.map(|paid| late(&paid, cause));
        self.advance();
        loss
    }

    /// Our Prepare `request_id` expired and waits for the hub. Returns the
    /// fulfillment at once if the hub paid it already.
    pub(super) fn wait(&mut self, request_id: u32) -> Option<Option<[u8; 32]>> {
        let sequence = *self.requests.get(&request_id)?;
        let packet = self.packets.get_mut(&sequence)?;
        if packet.state != State::InFlight {
            return None;
        }
        if let Some(paid) = packet.paid {
            packet.state = State::Decided;
            packet.request_id = None;
            self.requests.remove(&request_id);
            self.advance();
            return Some(Some(paid.fulfillment));
        }
        packet.state = State::Waiting;
        None
    }

    /// A final block stamped `timestamp`, up to which we read everything
    /// on the hold: the waits whose commitment expired by then are decided
    /// without a fulfillment, by request, and what can no longer be
    /// claimed is forgotten.
    pub(super) fn final_block(&mut self, timestamp: Millis) -> Vec<u32> {
        let mut decided = Vec::new();
        for packet in self.packets.values_mut() {
            if packet.state == State::Waiting && packet.commitment.expiry <= timestamp {
                packet.state = State::Decided;
                if let Some(request_id) = packet.request_id.take() {
                    decided.push(request_id);
                }
            }
        }
        for request_id in &decided {
            self.requests.remove(request_id);
        }
        self.advance();
        self.forget(timestamp);
        decided
    }

    /// The peer won a claim on the hub with `fulfilled`, which paid
    /// `paid`, in operation `operation`. Returns the waits it decides, with
    /// their fulfillments, and the losses it caused.
    pub(super) fn claimed(
        &mut self,
        fulfilled: &[Fulfilled],
        paid: Amount,
        operation: Digest,
    ) -> (Vec<(u32, [u8; 32])>, Vec<Loss>) {
        let mut decided = Vec::new();
        let mut losses = Vec::new();
        for (sequence, fulfillment, drops) in paid_in_order(fulfilled, paid) {
            let paid = PaidOnHub {
                fulfillment,
                drops,
                operation,
            };
            // A sequence the hub took is never ours to give again.
            self.next_sequence = self.next_sequence.max(sequence.saturating_add(1));
            match self.packets.get_mut(&sequence) {
                Some(packet) if packet.paid.is_none() => {
                    let was_owed = packet.owed();
                    packet.paid = Some(paid);
                    match packet.state {
                        State::Waiting => {
                            packet.state = State::Decided;
                            if let Some(request_id) = packet.request_id.take() {
                                decided.push((request_id, fulfillment));
                            }
                        }
                        State::Rejected => losses.push(late(&paid, LossCause::Cheated)),
                        State::GaveUp | State::Fulfilled { taken: false } => {
                            losses.push(late(&paid, LossCause::PaidLate));
                        }
                        _ => {}
                    }
                    if sequence <= self.through && !was_owed {
                        self.through_units = self.through_units.saturating_add(packet.owed_units());
                    }
                }
                Some(_) => {}
                None => {
                    // Forgotten, or never ours to know: the hub says what it
                    // paid, and we owe it.
                    losses.push(late(&paid, LossCause::PaidLate));
                    let units = u128::from(drops) * u128::from(UNITS_PER_DROP);
                    if sequence <= self.through {
                        self.through_units = self.through_units.saturating_add(units);
                    }
                }
            }
        }
        for (request_id, _) in &decided {
            self.requests.remove(request_id);
        }
        self.advance();
        (decided, losses)
    }

    /// Moves W past every resolved packet, adding what we owe for each.
    fn advance(&mut self) {
        loop {
            let next = self.through.saturating_add(1);
            if next >= self.next_sequence {
                return;
            }
            let units = match self.packets.get(&next) {
                Some(packet) if !packet.state.resolved() => return,
                Some(packet) => packet.owed_units(),
                None => 0,
            };
            self.through = next;
            self.through_units = self.through_units.saturating_add(units);
        }
    }

    /// Forgets what the hub can no longer pay: packets up to W whose
    /// commitment expired by a final block stamped `timestamp`, or that a
    /// redeem passed.
    fn forget(&mut self, timestamp: Millis) {
        let through = self.through;
        let redeemed = self.snapshot.through;
        self.packets.retain(|sequence, packet| {
            let gone = *sequence <= through
                && packet.state.resolved()
                && (packet.commitment.expiry <= timestamp || *sequence <= redeemed);
            !gone
        });
    }

    /// Asserts how its parts relate.
    pub(super) fn check_invariants(&self) {
        assert!(self.packets.len() <= MAX_PACKETS);
        assert!(self.next_sequence >= 1);
        assert!(
            self.through < self.next_sequence,
            "W below the next sequence"
        );
        assert!(
            self.packets
                .keys()
                .all(|sequence| *sequence < self.next_sequence),
            "every packet has a sequence given out"
        );
        for (request_id, sequence) in &self.requests {
            let packet = self.packets.get(sequence);
            assert!(
                packet.is_some_and(|p| p.request_id == Some(*request_id)),
                "a request names its packet"
            );
        }
        assert!(
            self.packets
                .range(..=self.through)
                .all(|(_, packet)| packet.state.resolved()),
            "every packet up to W is resolved"
        );
    }
}

/// What a claim paid each of its commitments: in sequence order, each as
/// far as what was paid covers it, as the hub pays them.
fn paid_in_order(fulfilled: &[Fulfilled], paid: Amount) -> Vec<(u64, [u8; 32], Amount)> {
    let mut ordered: Vec<(u64, [u8; 32], Amount)> = fulfilled
        .iter()
        .map(|item| {
            let commitment = &item.commitment.value;
            (commitment.sequence, item.fulfillment, commitment.amount)
        })
        .collect();
    ordered.sort_unstable_by_key(|(sequence, _, _)| *sequence);
    let mut left = paid;
    for (_, _, amount) in &mut ordered {
        let pay = (*amount).min(left);
        left = left.saturating_sub(pay);
        *amount = pay;
    }
    ordered
}

/// The loss of a packet the hub paid that our books did not count: what
/// it paid, in internal units.
fn late(paid: &PaidOnHub, cause: LossCause) -> Loss {
    Loss {
        units: super::units(paid.drops),
        cause,
        operation: paid.operation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promise::Signed;
    use dassie_identity::Signature;
    use sha2::{Digest as _, Sha256};

    const MARGIN: Millis = 500;

    fn payer(amount: Amount) -> Payer {
        let mut payer = Payer::new();
        payer.hold = Some(HoldNumber(7));
        payer.snapshot.amount = amount;
        payer
    }

    fn promised(units: u64, preimage: u8) -> Promised {
        Promised {
            amount: units,
            condition: Sha256::digest([preimage; 32]).into(),
            expires_at_ms: 10_000,
        }
    }

    fn fulfilled(commitment: Commitment, preimage: u8) -> Fulfilled {
        Fulfilled {
            commitment: Signed {
                value: commitment,
                signature: Signature::from_bytes([0; 64]),
            },
            fulfillment: [preimage; 32],
        }
    }

    // spec/designs/settlement-hubs.md#4-peering-over-a-hub: a commitment is
    // the Prepare's own, its expiry the margin earlier, its amount in drops
    // never less than the packet.
    #[test]
    fn a_commitment_promises_the_prepare_less_the_margin() {
        let mut payer = payer(1_000);
        let commitment = payer.commit(1, promised(1_500, 1), MARGIN).unwrap();
        assert_eq!(commitment.hold, HoldNumber(7));
        assert_eq!(commitment.sequence, 1);
        assert_eq!(commitment.amount, 2, "1.5 drops are promised as 2");
        assert_eq!(commitment.expiry, 10_000 - MARGIN);
        let next = payer.commit(2, promised(1_000, 2), MARGIN).unwrap();
        assert_eq!(next.sequence, 2, "sequences increase");
        assert_eq!(
            payer.commit(2, promised(1_000, 2), MARGIN),
            Err(SchemeError::InvalidData),
            "a request once"
        );
        payer.check_invariants();
    }

    #[test]
    fn a_hold_without_room_takes_no_packet() {
        let mut payer = payer(3);
        payer.commit(1, promised(2_000, 1), MARGIN).unwrap();
        assert_eq!(
            payer.commit(2, promised(1_001, 2), MARGIN),
            Err(SchemeError::Unavailable),
            "two drops in flight, one left"
        );
        payer.commit(2, promised(1_000, 2), MARGIN).unwrap();
        let mut none = Payer::new();
        assert_eq!(
            none.commit(1, promised(1, 1), MARGIN),
            Err(SchemeError::Unavailable),
            "no hold yet"
        );
    }

    // T covers exactly the packets up to W: W waits for every packet below
    // it, and a Fulfill not taken adds nothing.
    #[test]
    fn a_claim_covers_what_we_took_up_to_w() {
        let mut payer = payer(1_000_000);
        for request in 1..=3 {
            payer.commit(request, promised(2_500, 1), MARGIN).unwrap();
        }
        let (claim, _) = payer.fulfilled(2, true);
        assert_eq!((claim.unwrap().total, claim.unwrap().through), (0, 0));
        let (claim, _) = payer.fulfilled(1, true);
        assert_eq!((claim.unwrap().total, claim.unwrap().through), (5, 2));
        let (claim, _) = payer.fulfilled(3, false);
        assert_eq!((claim.unwrap().total, claim.unwrap().through), (5, 3));
        // An answer after the claim changes nothing.
        assert_eq!(payer.answered(3), None);
        payer.check_invariants();
    }

    // What is less than a drop stays owed until there is more.
    #[test]
    fn t_rounds_down_what_the_packets_add_up_to() {
        let mut payer = payer(1_000);
        payer.commit(1, promised(1_500, 1), MARGIN).unwrap();
        payer.commit(2, promised(1_500, 1), MARGIN).unwrap();
        assert_eq!(payer.fulfilled(1, true).0.unwrap().total, 1);
        assert_eq!(payer.fulfilled(2, true).0.unwrap().total, 3);
    }

    // spec/designs/settlement-hubs.md#4-peering-over-a-hub: a packet that
    // waits is decided by the hub: with the claim's fulfillment, or without
    // once a final block is stamped at or after its commitment's expiry.
    #[test]
    fn a_wait_is_decided_by_the_hub() {
        let mut payer = payer(1_000);
        let first = payer.commit(1, promised(1_000, 1), MARGIN).unwrap();
        payer.commit(2, promised(1_000, 2), MARGIN).unwrap();
        assert_eq!(payer.wait(1), None);
        assert_eq!(payer.wait(2), None);
        let (decided, losses) = payer.claimed(&[fulfilled(first, 1)], 1, Digest([9; 32]));
        assert_eq!(decided, [(1, [1; 32])]);
        assert!(losses.is_empty());
        assert_eq!(payer.final_block(first.expiry - 1), Vec::<u32>::new());
        assert_eq!(payer.final_block(first.expiry), [2]);
        assert_eq!(payer.claim_value().unwrap().total, 1, "the hub overrules");
        assert_eq!(payer.through, 2);
        payer.check_invariants();
    }

    #[test]
    fn a_claim_the_hub_paid_before_the_wait_decides_it_at_once() {
        let mut payer = payer(1_000);
        let first = payer.commit(1, promised(1_000, 1), MARGIN).unwrap();
        payer.claimed(&[fulfilled(first, 1)], 1, Digest([9; 32]));
        assert_eq!(payer.wait(1), Some(Some([1; 32])));
    }

    // spec/designs/settlement-hubs.md#13-how-it-fits-the-node: what the hub
    // pays for a packet our books did not count is a loss on the peer; for
    // one the peer rejected, evidence of cheating.
    #[test]
    fn what_the_hub_paid_behind_our_books_is_a_loss() {
        let mut payer = payer(1_000);
        let rejected = payer.commit(1, promised(1_000, 1), MARGIN).unwrap();
        let given_up = payer.commit(2, promised(2_000, 2), MARGIN).unwrap();
        assert_eq!(payer.answered(1), None);
        payer.wait(2);
        assert_eq!(payer.answered(2), None);
        let (_, losses) = payer.claimed(
            &[fulfilled(rejected, 1), fulfilled(given_up, 2)],
            3,
            Digest([4; 32]),
        );
        let causes: Vec<(u64, LossCause)> = losses.iter().map(|l| (l.units, l.cause)).collect();
        assert_eq!(
            causes,
            [(1_000, LossCause::Cheated), (2_000, LossCause::PaidLate)]
        );
        // Both count in T now: the hub paid them.
        assert_eq!(payer.claim_value().unwrap().total, 3);
        payer.check_invariants();
    }

    // A claim the hub paid while the packet was in flight is a loss only
    // once the peer rejects it.
    #[test]
    fn a_reject_after_the_hub_paid_is_cheating() {
        let mut payer = payer(1_000);
        let packet = payer.commit(1, promised(1_000, 1), MARGIN).unwrap();
        let (_, losses) = payer.claimed(&[fulfilled(packet, 1)], 1, Digest([4; 32]));
        assert!(losses.is_empty());
        let loss = payer.answered(1).unwrap();
        assert_eq!((loss.units, loss.cause), (1_000, LossCause::Cheated));
    }

    #[test]
    fn resolved_packets_are_forgotten_once_the_hub_can_no_longer_pay() {
        let mut payer = payer(1_000);
        let packet = payer.commit(1, promised(1_000, 1), MARGIN).unwrap();
        payer.fulfilled(1, true);
        payer.final_block(packet.expiry - 1);
        assert_eq!(payer.packets.len(), 1);
        payer.final_block(packet.expiry);
        assert!(payer.packets.is_empty());
        payer.check_invariants();
    }
}
