//! The interface every settlement scheme implements.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use dassie_ledger::{LedgerId, Name};
use rand_core::CryptoRng;

use crate::stub::Stub;

/// Something the node must book or do because of a scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A peer settled with us: Dr `assets/settlement`, Cr
    /// `assets/interledger/<peer>`, posted.
    IncomingSettlement {
        /// The peer.
        peer: Name,
        /// How much, in internal units.
        amount: u64,
    },
    /// Our outgoing settlement executed: post its pending transfer.
    Finalize {
        /// The settlement.
        settlement: Name,
    },
    /// Our outgoing settlement failed for good: void its pending transfer.
    /// The original never issued this, so failures held their amount until
    /// a restart (ACC-16).
    Cancel {
        /// The settlement.
        settlement: Name,
    },
    /// The owner added money: Dr `assets/settlement`, Cr `equity/owner`.
    Deposit {
        /// How much.
        amount: u64,
    },
    /// The owner took money out: Dr `equity/owner`, Cr `assets/settlement`.
    Withdrawal {
        /// How much.
        amount: u64,
    },
    /// Send a scheme message to a peer.
    SendMessage {
        /// The peer.
        peer: Name,
        /// The scheme's payload.
        message: Vec<u8>,
    },
}

/// An outgoing settlement, prepared but not yet executed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSettlement {
    /// Unique within the scheme. The node keys the pending transfer with
    /// it, durably.
    pub id: Name,
    /// The proof or claim to send the peer with the settlement message.
    pub message: Vec<u8>,
    /// The peer's new scheme state, if it changed. The node keeps it; the
    /// original dropped it (ACC-15).
    pub peer_state: Option<Vec<u8>>,
}

/// Why a scheme refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeError {
    /// No scheme has that ID.
    UnknownScheme,
    /// The scheme does not accept that config.
    InvalidConfig,
    /// The scheme does not do that, for example deposits from the node.
    Unsupported,
    /// A peer's peering or settlement data is invalid.
    InvalidData,
    /// The scheme has too many settlements prepared and not executed.
    TooManyPrepared,
    /// A balance would overflow.
    Overflow,
}

impl fmt::Display for SchemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownScheme => "unknown settlement scheme",
            Self::InvalidConfig => "invalid settlement scheme config",
            Self::Unsupported => "the settlement scheme does not support this",
            Self::InvalidData => "invalid settlement data",
            Self::TooManyPrepared => "too many settlements in preparation",
            Self::Overflow => "the balance would overflow",
        })
    }
}

/// A settlement scheme (spec 5 §4.3), sans-IO.
///
/// Peering hooks and settlement are used from M4, when nodes peer; M3 runs
/// the stub's deposits.
pub trait SettlementScheme: Send {
    /// The scheme's ID, as advertised to peers, for example `stub`.
    fn id(&self) -> Name;

    /// The internal ledger the scheme's money is on.
    fn ledger(&self) -> LedgerId;

    /// The scheme protocol versions it speaks, negotiated when peering
    /// (ACC-14).
    fn supported_versions(&self) -> &'static [u16];

    /// The balance on the underlying ledger, in internal units: what the
    /// node checkpoints.
    fn balance(&self) -> u64;

    /// What a node that wants to peer with us needs to know.
    fn peering_info(&self, out: &mut Vec<u8>);

    /// Builds our peering request to `peer`, given its peering info.
    ///
    /// # Errors
    ///
    /// If the info is invalid.
    fn create_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError>;

    /// Accepts or refuses `peer`'s request, writing the response data.
    /// Returns the peer's scheme state.
    ///
    /// # Errors
    ///
    /// If the request is refused.
    fn accept_peering_request(
        &mut self,
        peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError>;

    /// Completes our request after `peer` accepted it. Returns the peer's
    /// scheme state.
    ///
    /// # Errors
    ///
    /// If the response is invalid.
    fn finalize_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        data: &[u8],
    ) -> Result<Vec<u8>, SchemeError>;

    /// Prepares a settlement of `amount` to `peer`. Submits nothing.
    ///
    /// # Errors
    ///
    /// If the scheme cannot settle now.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError>;

    /// Executes a prepared settlement. Reports the outcome as
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later. The node
    /// calls it only once the settlement's pending transfer is on disk.
    fn execute_settlement(&mut self, settlement: Name, effects: &mut Vec<Effect>);

    /// Settles the outcome of settlements whose pending transfers survived
    /// a restart, given by ID and amount, at activation (ACC-16): each gets
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later.
    fn reconcile(&mut self, pending: &[(Name, u64)], effects: &mut Vec<Effect>);

    /// Handles a peer's settlement message.
    ///
    /// # Errors
    ///
    /// If the data is invalid.
    fn handle_settlement(
        &mut self,
        peer: Name,
        amount: u64,
        data: &[u8],
        peer_state: &[u8],
        effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError>;

    /// Handles a scheme message from a peer.
    fn handle_message(&mut self, peer: Name, message: &[u8], effects: &mut Vec<Effect>);

    /// Adds money on the owner's behalf, where the scheme can.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] where the owner deposits on the
    /// underlying ledger itself.
    fn deposit(&mut self, amount: u64, effects: &mut Vec<Effect>) -> Result<(), SchemeError>;
}

/// A scheme [`create`] can make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemeInfo {
    /// Its ID, which [`create`] takes.
    pub id: &'static str,
    /// Its name for people.
    pub name: &'static str,
    /// The ledger it serves.
    pub ledger: LedgerId,
}

/// Every scheme [`create`] can make, in a fixed order.
pub const SCHEMES: &[SchemeInfo] = &[SchemeInfo {
    id: "stub",
    name: "Stub",
    ledger: LedgerId::StubUsd,
}];

/// The scheme `id`, set up with `config` and the balance the books last
/// checkpointed for it, if any. The original stored config and never passed
/// it on (ACC-12).
///
/// # Errors
///
/// If the scheme is unknown or refuses the config.
pub fn create(
    id: &str,
    config: &str,
    checkpoint: Option<u64>,
) -> Result<Box<dyn SettlementScheme>, SchemeError> {
    match id {
        "stub" => Ok(Box::new(Stub::new(config, checkpoint)?)),
        _ => Err(SchemeError::UnknownScheme),
    }
}
