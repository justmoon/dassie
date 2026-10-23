//! Schemes whose peerings carry signed promises with every packet, as a
//! settlement hub's do (spec/designs/settlement-hubs.md#4-peering-over-a-hub):
//! the payer signs a commitment with each Prepare and a claim with its
//! reply to each Fulfill, and the payee can take what it is owed with
//! either, without the payer's help.
//!
//! The node reaches this side of a scheme through
//! [`SettlementScheme::commitments`](crate::SettlementScheme::commitments).
//! A scheme without it settles as every other: the node never asks.

use alloc::vec::Vec;

use dassie_ledger::Name;

use crate::scheme::SchemeError;

/// What a commitment promises: a Prepare's amount, condition and expiry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Promised {
    /// The Prepare's amount, in the ledger's internal units.
    pub amount: u64,
    /// Its execution condition.
    pub condition: [u8; 32],
    /// Its expiry, in milliseconds since the Unix epoch.
    pub expires_at_ms: u64,
}

/// How the hub decided a packet whose Reject upstream waited for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decided {
    /// The peer the packet went to.
    pub peer: Name,
    /// The request ID it went with.
    pub request_id: u32,
    /// The fulfillment the peer's claim on the hub revealed, if it won
    /// one before the commitment's expiry; `None` once a final block
    /// stamped at or after the expiry shows it did not.
    pub fulfillment: Option<[u8; 32]>,
}

/// The side of a scheme that signs and takes promises per packet.
pub trait Commitments {
    /// As the payer: the commitment to send with our Prepare `request_id`
    /// to `peer`. Refused when the peer's room in our hold cannot take the
    /// packet: the node then rejects it as it rejects one beyond a peer's
    /// credit.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] while the hold cannot cover it.
    fn commit(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError>;

    /// As the payee: checks `peer`'s commitment for its Prepare
    /// `request_id`, which we forward only if it holds: signed by the
    /// payer's key, for its hold for us, for what the Prepare says, and
    /// within what the hold covers beyond what we have not taken.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidData`] for a commitment that does not hold;
    /// [`SchemeError::Unavailable`] while the hold cannot cover it.
    fn check_commitment(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        commitment: &[u8],
    ) -> Result<(), SchemeError>;

    /// As the payee: we fulfilled `peer`'s Prepare `request_id`. Kept until
    /// the payer's claim covers it, so a missing claim can be taken on the
    /// hub with the commitment and this fulfillment.
    fn on_fulfilled(&mut self, now_ms: u64, peer: Name, request_id: u32, fulfillment: [u8; 32]);

    /// As the payee: `peer`'s Prepare `request_id` was rejected or expired:
    /// its commitment is forgotten.
    fn on_released(&mut self, peer: Name, request_id: u32);

    /// As the payer: the claim to send after the verdict that accepts
    /// `peer`'s Fulfill of our Prepare `request_id`, written to `out`.
    /// `taken` says whether the books took the Fulfill: a Fulfill we did
    /// not take adds nothing to the total we claim to owe.
    fn claim(&mut self, now_ms: u64, peer: Name, request_id: u32, taken: bool, out: &mut Vec<u8>);

    /// As the payee: the claim `peer` sent with its reply to our Fulfill of
    /// its Prepare `request_id`. A claim that does not hold is dropped:
    /// the scheme takes what it is owed on the hub instead.
    fn on_claim(&mut self, now_ms: u64, peer: Name, request_id: u32, claim: &[u8]);

    /// As the payer: our Prepare `request_id` to `peer` expired and waits
    /// for the hub to decide it, until `backstop_ms`.
    fn on_hub_wait(&mut self, now_ms: u64, peer: Name, request_id: u32, condition: [u8; 32], backstop_ms: u64);

    /// As the payer: our Prepare `request_id` to `peer` was answered, or
    /// decided: what the scheme kept for it is forgotten.
    fn on_answered(&mut self, peer: Name, request_id: u32);

    /// Moves the waits the hub has decided into `out`, oldest first.
    fn take_decided(&mut self, out: &mut Vec<Decided>);
}
